use std::ops::Rem;
use std::ptr::NonNull;

use crate::errors::SqliteError;

use super::buffer_pool::{self, BufferPool};
use super::frame::FrameId;
use super::frame::{CLEAN, DIRTY, REFERENCED};
use super::guard::{BorrowState, PageGuard};
use super::metadata::SqliteMetadata;
use super::statistics::SqliteStatistics;
use crate::DbError;
use crate::PageNo;
use crate::format::page::{self};
use crate::pager::frame::Frame;
use crate::vfs::file::SqliteFile;

// #[derive(Debug)]
pub struct Pager {
    pub source: Box<dyn SqliteFile>,
    pub buffer_pool: BufferPool,
    // dirty pages linked list instead of new allocations
    pub dp_ll: Option<FrameId>,
    pub metadata: SqliteMetadata,
    pub statistics: SqliteStatistics, // pub configuration: SqliteConfiguration,
}

impl Pager {
    pub fn new(
        source: impl SqliteFile + 'static,
        page_size: usize,
        usable_size: usize,
        max_allocated_pages: usize,
    ) -> Self {
        Self {
            source: Box::new(source),
            buffer_pool: BufferPool::new(page_size),
            dp_ll: None,
            metadata: SqliteMetadata::new(page_size, usable_size, max_allocated_pages),
            statistics: SqliteStatistics::default(),
        }
    }

    pub fn with_cache(
        source: impl SqliteFile + 'static,
        page_size: usize,
        usable_size: usize,
        max_allocated_pages: usize,
        cache_size: usize,
    ) -> Self {
        Self {
            source: Box::new(source),
            buffer_pool: BufferPool::with_cache(cache_size, page_size),
            dp_ll: None,
            metadata: SqliteMetadata::new(page_size, usable_size, max_allocated_pages),
            statistics: SqliteStatistics::default(),
        }
    }

    // PageGuard holds lifetime of self
    pub fn get(&mut self, page_no: PageNo) -> Result<PageGuard, DbError> {
        Self::validate_page(
            page_no,
            self.metadata.max_allocated_pages,
            None::<fn(_) -> bool>,
        )?;
        self.ensure_page_loaded(page_no)?;
        Ok(self
            .try_get_fast(page_no)
            .expect("page should be present after get_impl"))
    }

    pub fn get_mut(&mut self, page_no: PageNo) -> Result<PageGuard, DbError> {
        Self::validate_page(
            page_no,
            self.metadata.max_allocated_pages,
            None::<fn(_) -> bool>,
        )?;
        let was_dirty = self.ensure_page_loaded(page_no)?;
        Ok(self
            .try_get_fast_mut(page_no, was_dirty)
            .expect("page should be present after get_impl"))
    }

    // cache look up
    fn try_get_fast(&mut self, page_no: PageNo) -> Option<PageGuard> {
        if let Some(frame_id) = self.buffer_pool.page_table.get(&page_no) {
            let frame_id = *frame_id;
            let frame = &mut self.buffer_pool.frame_buffer[frame_id];
            frame.incr_pin_count();
            frame.set(REFERENCED);
            let page_guard = self.page_guard(frame_id);
            return Some(page_guard);
        }
        None
    }

    fn try_get_fast_mut(&mut self, page_no: PageNo, was_dirty: bool) -> Option<PageGuard> {
        if let Some(frame_id) = self.buffer_pool.page_table.get(&page_no) {
            let frame_id = *frame_id;
            let frame = &mut self.buffer_pool.frame_buffer[frame_id];
            frame.incr_pin_count();
            frame.clear(CLEAN);
            frame.set(REFERENCED | DIRTY);
            // This fixes the bug of inserting the same node twice
            // for example in call like
            // let p2_rc = get_mut(page_2);
            // let p2_rc_2 = get_mut(page_2);
            // this will insert the page twice
            // which can also cause infinite loop (pointer point to it self)
            if !was_dirty {
                self.dp_ll_insert(frame_id);
            }
            let page_guard = self.page_guard_mut(frame_id);
            return Some(page_guard);
        }
        None
    }
    // page not in cache
    fn ensure_page_loaded(&mut self, page_no: PageNo) -> Result<bool, DbError> {
        if let Some(frameid) = self.buffer_pool.page_table.get(&page_no) {
            let mut is_dirty = false;
            if self.buffer_pool.frame_buffer[*frameid].is(DIRTY) {
                is_dirty = true;
            }
            self.statistics.inc_cache_hit();
            return Ok(is_dirty); // page already in cache
        }

        // check if the we have any free frames
        if let Some(frameid) = self.buffer_pool.free_frames.pop() {
            self.buffer_pool.page_table.insert(page_no, frameid);

            self.allocate_page(page_no, frameid)?;

            let frame = &mut self.buffer_pool.frame_buffer[frameid];
            *frame = Frame::new(Some(page_no), CLEAN, 0);

            return Ok(false);
        }
        // run the clock
        let mut clock_hand = self.buffer_pool.clock_hand;
        let start = clock_hand;
        let mut laps = 0;
        let buffer_len = self.buffer_pool.frame_buffer.len();
        let frameid: usize = loop {
            if clock_hand == start {
                laps += 1;
                // TODO: Explain why more than 2 laps
                if laps > 2 {
                    return Err(DbError::BufferPoolExhausted);
                }
            }
            let frame = &mut self.buffer_pool.frame_buffer[clock_hand];
            if frame.pin_count.get() == 0 {
                if frame.is(REFERENCED) {
                    frame.clear(REFERENCED);
                } else {
                    break clock_hand;
                }
            }
            clock_hand = (clock_hand + 1) % buffer_len;
        };

        self.buffer_pool.clock_hand = clock_hand;
        let frame = &self.buffer_pool.frame_buffer[frameid];
        let frame_page_no = frame.page_no.unwrap();
        // if the frame is dirty, flush it to the disk first
        if frame.is(DIRTY) {
            self.flush_page(frame_page_no, frameid)?;
            self.dp_ll_remove(frameid);
        }

        // evict the page from page table
        self.buffer_pool.evict_page(frame_page_no, frameid)?;
        self.statistics.inc_evictions();

        self.allocate_page(page_no, frameid)?;

        // after the alloation; set the new frame
        //
        self.buffer_pool.frame_buffer[frameid] = Frame::new(Some(page_no), CLEAN, 0);
        Ok(false)
    }
    fn allocate_page(&mut self, page_no: PageNo, frameid: FrameId) -> Result<(), DbError> {
        // request memory
        let page_offset = self.get_page_offset(page_no);
        let start = frameid * self.metadata.page_size;
        let end = start + self.metadata.page_size;
        let page_buffer = &mut self.buffer_pool.page_buffer[start..end];
        // if this went right
        self.source.read_exact_at(page_offset as _, page_buffer)?;
        self.buffer_pool.page_table.insert(page_no, frameid);

        self.statistics.inc_cache_miss();

        Ok(())
    }
    pub fn dp_ll_insert(&mut self, frame_id: FrameId) {
        let frame = &mut self.buffer_pool.frame_buffer[frame_id];
        frame.prev = self.dp_ll;
        frame.next = None;
        if let Some(db_ll_tail) = self.dp_ll {
            let ll_tail_frame = &mut self.buffer_pool.frame_buffer[db_ll_tail];
            ll_tail_frame.next = Some(frame_id);
        }
        self.dp_ll = Some(frame_id);
    }
    pub fn dp_ll_remove(&mut self, frame_id: FrameId) {
        let mut next = None;
        let mut prev = None;
        // safe to unwrap since we want to remove a Node,
        // so logically we at lease have one node
        let mut is_tail = frame_id == self.dp_ll.unwrap(); // if this went wrong, we have a bug

        // BORROWING HELL
        {
            let frame = &mut self.buffer_pool.frame_buffer[frame_id];
            next = frame.next;
            prev = frame.prev;
            // in case this returned the buffer pool,
            // should not handle its old pointers so it breaks the list
            frame.prev = None;
            frame.next = None;
        }
        if is_tail && prev.is_none() {
            self.dp_ll = None;
            return;
        }
        if let Some(next_frame_id) = next {
            let next_frame = &mut self.buffer_pool.frame_buffer[next_frame_id];
            next_frame.prev = prev
        }
        if let Some(prev_frame_id) = prev {
            let prev_frame = &mut self.buffer_pool.frame_buffer[prev_frame_id];
            prev_frame.next = next;
            if is_tail {
                self.dp_ll = Some(prev_frame_id);
            }
        }
    }

    fn get_page_offset(&self, page_no: PageNo) -> usize {
        ((page_no as usize) - 1) * self.metadata.page_size
    }
    fn page_guard(&mut self, frameid: FrameId) -> PageGuard {
        let start = frameid * self.metadata.page_size;
        let end = start + self.metadata.page_size;
        let buffer_pool = self.buffer_pool.as_ptr_mut();
        let ptr = unsafe {
            NonNull::new_unchecked(self.buffer_pool.page_buffer[start..end].as_ptr() as *mut u8)
        };
        let slice = NonNull::<[u8]>::slice_from_raw_parts(ptr, self.metadata.page_size);

        PageGuard::new(buffer_pool, frameid, slice, BorrowState::Ref)
    }
    fn page_guard_mut(&mut self, frameid: FrameId) -> PageGuard {
        let start = frameid * self.metadata.page_size;
        let end = start + self.metadata.page_size;
        let buffer_pool = self.buffer_pool.as_ptr_mut();
        // let bytes = self.buffer_pool.page_buffer[start..end].as_mut();
        let ptr = unsafe {
            NonNull::new_unchecked(self.buffer_pool.page_buffer[start..end].as_ptr() as *mut u8)
        };
        let slice = NonNull::<[u8]>::slice_from_raw_parts(ptr, self.metadata.page_size);
        PageGuard::new(buffer_pool, frameid, slice, BorrowState::RefMut)
    }
    fn flush_page(&self, page_no: PageNo, frameid: FrameId) -> Result<(), DbError> {
        let offset = self.get_page_offset(page_no);
        let start = frameid * self.metadata.page_size;
        let end = start + self.metadata.page_size;
        let bytes = &self.buffer_pool.page_buffer[start..end];
        self.buffer_pool.frame_buffer[frameid].reset_to(CLEAN);
        self.source.write_all_at(offset as _, bytes)?;
        self.statistics.inc_disk_write();
        self.source.sync()?; // temporary for now !!
        Ok(())
    }
    pub fn flush_all(&mut self) -> Result<(), SqliteError> {
        let mut tail = self.dp_ll;
        while let Some(tail_f_id) = tail {
            let frame = &self.buffer_pool.frame_buffer[tail_f_id];
            let page_no = frame.page_no.unwrap();
            debug_assert!(
                frame.is(DIRTY),
                "Page {} is not dirty while its declared as dirty in the linked list",
                page_no
            );
            self.flush_page(page_no, tail_f_id)?;
            tail = frame.prev;
            self.dp_ll_remove(tail_f_id);
        }
        Ok(())
    }

    pub fn validate_page<E>(
        page_no: PageNo,
        max_pages: usize,
        exception: Option<E>,
    ) -> Result<(), DbError>
    where
        E: Fn(PageNo) -> bool,
    {
        if let Some(exc) = exception
            && exc(page_no)
        {
            return Err(SqliteError::Corrupt("Exception Failed".into()));
        }
        if page_no == 0 {
            return Err(SqliteError::Corrupt("page number cannot be zero".into()));
        } else if page_no as usize > max_pages {
            return Err(SqliteError::Corrupt(
                "page number is outside the database".into(),
            ));
        }

        Ok(())
    }
    pub fn cached_page_count(&self) -> usize {
        self.buffer_pool.frame_buffer.len() - self.buffer_pool.free_frames.len()
    }
}

impl Drop for Pager {
    fn drop(&mut self) {
        self.flush_all();
    }
}

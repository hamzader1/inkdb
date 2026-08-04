use std::ptr::NonNull;

use crate::errors::SqliteError;

use super::buffer_pool::BufferPool;
use super::frame::FrameId;
use super::frame::{CLEAN, DIRTY, REFERENCED};
use super::guard::{PageGuard, PageGuardMut};
use super::metadata::SqliteMetadata;
use crate::format::page::PageNo;
use crate::pager::frame::Frame;
use crate::vfs::file::SqliteFile;
use crate::DbError;

#[derive(Debug)]
pub struct Pager<F: SqliteFile> {
    pub source: F,
    pub buffer_pool: BufferPool,
    pub metadata: SqliteMetadata,
}

impl<F: SqliteFile> Pager<F> {
    pub fn new(
        source: F,
        page_size: usize,
        usable_size: usize,
        max_allocated_pages: usize,
    ) -> Self {
        Self {
            source,
            buffer_pool: BufferPool::new(page_size),
            metadata: SqliteMetadata::new(page_size, usable_size, max_allocated_pages),
        }
    }

    pub fn with_cache(
        source: F,
        page_size: usize,
        usable_size: usize,
        max_allocated_pages: usize,
        cache_size: usize,
    ) -> Self {
        Self {
            source,
            buffer_pool: BufferPool::with_cache(cache_size, page_size),
            metadata: SqliteMetadata::new(page_size, usable_size, max_allocated_pages),
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

    pub fn get_mut(&mut self, page_no: PageNo) -> Result<PageGuardMut, DbError> {
        Self::validate_page(
            page_no,
            self.metadata.max_allocated_pages,
            None::<fn(_) -> bool>,
        )?;
        self.ensure_page_loaded(page_no)?;
        Ok(self
            .try_get_fast_mut(page_no)
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

    fn try_get_fast_mut(&mut self, page_no: PageNo) -> Option<PageGuardMut> {
        if let Some(frame_id) = self.buffer_pool.page_table.get(&page_no) {
            let frame_id = *frame_id;
            let frame = &mut self.buffer_pool.frame_buffer[frame_id];
            frame.incr_pin_count();
            frame.clear(CLEAN);
            frame.set(REFERENCED | DIRTY);
            let page_guard = self.page_guard_mut(frame_id);
            return Some(page_guard);
        }
        None
    }
    // page not in cache
    fn ensure_page_loaded(&mut self, page_no: PageNo) -> Result<(), DbError> {
        if self.buffer_pool.page_table.contains_key(&page_no) {
            return Ok(()); // page already in cache
        }

        // check if the we have any free frames
        if let Some(frameid) = self.buffer_pool.free_frames.pop() {
            self.buffer_pool.page_table.insert(page_no, frameid);
            let frame = &mut self.buffer_pool.frame_buffer[frameid];
            *frame = Frame::new(Some(page_no), CLEAN, 0);

            self.load_page(page_no, frameid)?;
            return Ok(());
        }
        // run the clock
        let mut clock_hand = self.buffer_pool.clock_hand;
        let start = clock_hand;
        let mut laps = 0;
        let last_index = self.buffer_pool.frame_buffer.len();
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
        }

        // evict the page from page table
        self.buffer_pool.evict_page(frame_page_no, frameid)?;

        // set the new frame
        self.buffer_pool.frame_buffer[frameid] = Frame::new(Some(page_no), CLEAN, 0);

        self.load_page(page_no, frameid)?;
        Ok(())
    }
    fn load_page(&mut self, page_no: PageNo, frameid: FrameId) -> Result<(), DbError> {
        // request memory
        let page_offset = self.get_page_offset(page_no);
        let page_buffer =
            &mut self.buffer_pool.page_buffer[frameid..frameid + self.metadata.page_size];
        // if this went right
        self.source.read_exact_at(page_offset as _, page_buffer)?;
        self.buffer_pool.page_table.insert(page_no, frameid);

        Ok(())
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
        let slice: NonNull<[u8]> = NonNull::slice_from_raw_parts(ptr, self.metadata.page_size);

        PageGuard::new(buffer_pool, frameid, slice)
    }
    fn page_guard_mut(&mut self, frameid: FrameId) -> PageGuardMut {
        let start = frameid * self.metadata.page_size;
        let end = start + self.metadata.page_size;
        let buffer_pool = self.buffer_pool.as_ptr_mut();
        // let bytes = self.buffer_pool.page_buffer[start..end].as_mut();
        let ptr = unsafe {
            NonNull::new_unchecked(self.buffer_pool.page_buffer[start..end].as_ptr() as *mut u8)
        };
        let slice: NonNull<[u8]> = NonNull::slice_from_raw_parts(ptr, self.metadata.page_size);
        PageGuardMut::new(buffer_pool, frameid, slice)
    }
    fn flush_page(&self, page_no: PageNo, frameid: FrameId) -> Result<(), DbError> {
        let offset = self.get_page_offset(page_no);
        let start = frameid * self.metadata.page_size;
        let end = start + self.metadata.page_size;
        let bytes = &self.buffer_pool.page_buffer[start..end];
        self.source.write_all_at(offset as _, bytes)?;
        self.source.sync()?;
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
        if let Some(exc) = exception {
            if exc(page_no) {
                return Err(SqliteError::Corrupt("Exception Failed".into()));
            }
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
}

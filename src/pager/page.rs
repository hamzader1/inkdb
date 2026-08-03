use std::ptr::NonNull;

use super::buffer_pool::BufferPool;
use super::frame::FrameId;
use super::frame::{CLEAN, DIRTY, FREE, REFERENCED};
use super::guard::{PageGuard, PageGuardMut};
use super::metadata::SqliteMetadata;
use crate::format::page::PageNo;
use crate::pager::frame::Frame;
use crate::pager::page;
use crate::vfs::file::SqliteFile;
use crate::{sqlite_assert_all, DbError};

struct Pager<F: SqliteFile> {
    source: F,
    buffer_pool: BufferPool,
    metadata: SqliteMetadata,
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

    // PageGuard hold lifetime of self
    pub fn get(&mut self, page_no: PageNo) -> Result<PageGuard<'_>, DbError> {
        self.get_impl(page_no)?;
        Ok(self
            .try_get_fast(page_no)
            .unwrap_or_else(|| panic!("Failed to get page from page table")))
    }

    pub fn get_mut(&mut self, page_no: PageNo) -> Result<PageGuardMut<'_>, DbError> {
        self.get_impl(page_no)?;
        Ok(self
            .try_get_fast_mut(page_no)
            .unwrap_or_else(|| panic!("Failed to get page from page table")))
    }

    // cache look up
    fn try_get_fast(&mut self, page_no: PageNo) -> Option<PageGuard<'_>> {
        if let Some(frame_id) = self.buffer_pool.page_table.get(&page_no) {
            let frame_id = *frame_id;
            let frame = &mut self.buffer_pool.frame_buffer[frame_id];
            // increment the pin counter
            frame.incr_pin_count();
            // set REFERENCED to 1
            frame.set(REFERENCED);
            let page_guard = self.page_guard(frame_id);
            return Some(page_guard);
        }
        None
    }

    fn try_get_fast_mut(&mut self, page_no: PageNo) -> Option<PageGuardMut<'_>> {
        if let Some(frame_id) = self.buffer_pool.page_table.get(&page_no) {
            let frame_id = *frame_id;
            let frame = &mut self.buffer_pool.frame_buffer[frame_id];
            // increment the pin counter
            frame.incr_pin_count();
            // set REFERENCED to 1
            frame.set(REFERENCED | DIRTY);
            let page_guard = self.page_guard_mut(frame_id);
            return Some(page_guard);
        }
        None
    }
    // page not in cache
    fn get_impl(&mut self, page_no: PageNo) -> Result<(), DbError> {
        if let Some(_) = self.buffer_pool.page_table.get(&page_no) {
            return Ok(());
        }
        // assum the page is valid

        // check if the we have any free frames
        if let Some(frameid) = self.buffer_pool.free_frames.pop() {
            self.buffer_pool.page_table.insert(page_no, frameid);
            let frame = &mut self.buffer_pool.frame_buffer[frameid];
            *frame = Frame::new(Some(page_no), CLEAN, 0);

            self.load_page(page_no, frameid);
            return Ok(());
        }
        // run the clock
        let mut clock_hand = self.buffer_pool.clock_hand;
        let start = clock_hand;
        let mut laps = 0;
        let last_index = self.buffer_pool.frame_buffer.len();
        let mut frameid = None;
        let buffer_len = self.buffer_pool.frame_buffer.len();
        loop {
            if clock_hand == start {
                laps += 1;
                // Explain why more than 2 laps
                if laps > 2 {
                    return Err(DbError::BufferPoolExhausted);
                }
            }
            let frame = &mut self.buffer_pool.frame_buffer[clock_hand];
            if frame.pin_count.get() == 0 {
                if frame.is(REFERENCED) {
                    frame.clear(REFERENCED);
                } else {
                    frameid = Some(clock_hand);
                    break;
                }
            }
            clock_hand = (clock_hand + 1) % buffer_len;
        }
        let frameid = frameid.unwrap();
        let frame = &self.buffer_pool.frame_buffer[frameid];
        let frame_page_no = frame.page_no.unwrap();
        // if the frame is dirty, flush it to the disk first
        if frame.is(DIRTY) {
            self.flush_page(frame_page_no, frameid)?;
        }

        // evict the page from page table
        self.buffer_pool.page_table.remove(&frame_page_no);

        // clean the frame
        self.buffer_pool.frame_buffer[frameid] = Frame::new(Some(page_no), CLEAN, 0);

        self.load_page(page_no, frameid);
        Ok(())
    }
    fn load_page(&mut self, page_no: PageNo, frameid: FrameId) -> Result<(), DbError> {
        // request memory
        let page_offset = self.get_page_offset(page_no);
        let page_buffer =
            &mut self.buffer_pool.page_buffer[frameid..frameid + self.metadata.page_size];
        // if this went right
        self.source.read_exact_at(page_offset as _, page_buffer);
        self.buffer_pool.page_table.insert(page_no, frameid);

        Ok(())
    }
    fn get_page_offset(&self, page_no: PageNo) -> usize {
        // TODO: add page validation
        sqlite_assert_all!(
            page_no > 0,
            page_no as usize <= self.metadata.max_allocated_pages
        );
        ((page_no as usize) - 1) * self.metadata.page_size
    }
    fn page_guard(&mut self, frameid: FrameId) -> PageGuard<'_> {
        let start = frameid;
        let end = start + self.metadata.page_size;
        let buffer_pool = self.buffer_pool.as_ptr_mut();
        let bytes = self.buffer_pool.page_buffer[start..end].as_ref();
        PageGuard::new(buffer_pool, frameid, bytes)
    }
    fn page_guard_mut(&mut self, frameid: FrameId) -> PageGuardMut<'_> {
        let start = frameid;
        let end = start + self.metadata.page_size;
        let buffer_pool = self.buffer_pool.as_ptr_mut();
        let bytes = self.buffer_pool.page_buffer[start..end].as_mut();
        PageGuardMut::new(buffer_pool, frameid, bytes)
    }
    fn flush_page(&self, page_no: PageNo, frameid: FrameId) -> Result<(), DbError> {
        let offset = self.get_page_offset(page_no);
        let bytes = &self.buffer_pool.page_buffer[frameid..frameid + self.metadata.page_size];
        self.source.write_all_at(offset as _, bytes)?;
        self.source.sync()?;
        Ok(())
    }
}

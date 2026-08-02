use std::ptr::NonNull;

use super::buffer_pool::BufferPool;
use super::frame::FrameId;
use super::frame::{CLEAN, DIRTY, FREE, REFERENCED};
use super::guard::{PageBytes, PageGuard};
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
    pub fn get(&mut self, page_no: PageNo) -> PageGuard<'_> {
        // page is cached
        // TODO: extract it to Fast Path
        todo!()
    }

    // cache look up
    fn try_get_fast(&mut self, page_no: PageNo) -> Option<PageGuard<'_>> {
        if let Some(frame_id) = self.buffer_pool.page_map.get(&page_no) {
            let frame_id = *frame_id;
            let frame = &mut self.buffer_pool.frame_buffer[frame_id];
            // increment the pin counter
            frame.incr_pin_count();
            // set REFERENCED to 1
            frame.set(REFERENCED);
            let page_guard = self.page_guard_ref(frame_id);
            return Some(page_guard);
        }
        None
    }
    // page not in cache
    fn try_get_slow(&mut self, page_no: PageNo) -> Result<PageGuard<'_>, DbError> {
        // assum the page is valid

        // check if the we have any free frames
        if let Some(frameid) = self.buffer_pool.free_frames.pop() {
            self.buffer_pool.page_map.insert(page_no, frameid);
            let frame = &mut self.buffer_pool.frame_buffer[frameid];
            *frame = Frame::new(Some(page_no), CLEAN, 0);

            // frame.reset_to(REFERENCED | CLEAN);
            let offset = self.get_page_offset(page_no);
            // let buffer_pool_ptr = self.buffer_pool.as_ptr_mut();
            let page_buffer =
                &mut self.buffer_pool.page_buffer[frameid..frameid + self.metadata.page_size];
            self.source.read_exact_at(offset as _, page_buffer);
            return Ok(self.try_get_fast(page_no).unwrap());
            // let page_guard = PageGuard::new_ref(buffer_pool_ptr, page_buffer, frameid);
            // return Ok(page_guard);
            // get memory
        }
        // run the clock
        let mut clock_hand = self.buffer_pool.clock_hand;
        let start = clock_hand;
        let mut laps = 0;
        let last_index = self.buffer_pool.frame_buffer.len();
        let mut frameid = None;
        // TODO: fix infinte loop case
        loop {
            if clock_hand == start {
                laps += 1;
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
            clock_hand = (clock_hand + 1) % self.buffer_pool.frame_buffer.len();
        }
        let frameid = frameid.unwrap();
        let frame = &self.buffer_pool.frame_buffer[frameid];
        let frame_page_no = frame.page_no.unwrap();
        if frame.is(DIRTY) {
            let offset = self.get_page_offset(frame_page_no);
            let bytes = &self.buffer_pool.page_buffer[frameid..frameid + self.metadata.page_size];
            self.source.write_all_at(offset as _, bytes)?;
            self.source.sync()?;
        }
        self.buffer_pool.page_map.remove(&frame_page_no);
        self.buffer_pool.frame_buffer[frameid] = Frame::new(Some(page_no), CLEAN, 0);
        self.buffer_pool.page_map.insert(page_no, frameid);

        // write new page
        let offset = self.get_page_offset(page_no);
        let page_buffer =
            &mut self.buffer_pool.page_buffer[frameid..frameid + self.metadata.page_size];
        self.source.read_exact_at(offset as _, page_buffer);
        Ok(self.try_get_fast(page_no).unwrap())
    }
    fn get_page_offset(&self, page_no: PageNo) -> usize {
        sqlite_assert_all!(
            page_no > 0,
            page_no as usize <= self.metadata.max_allocated_pages
        );
        ((page_no as usize) - 1) * self.metadata.page_size
    }
    fn page_guard_ref(&mut self, frameid: FrameId) -> PageGuard<'_> {
        let start = frameid;
        let end = start + self.metadata.page_size;
        let buffer_pool = self.buffer_pool.as_ptr_mut();
        let bytes = self.buffer_pool.page_buffer[start..end].as_ref();
        PageGuard::new_ref(buffer_pool, bytes, frameid)
    }
}

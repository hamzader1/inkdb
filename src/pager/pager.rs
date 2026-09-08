use std::collections::HashSet;
use std::ptr::NonNull;

use crate::db::header::{
    DATABASE_SIZE_IN_PAGES_OFFSET, DATABASE_SIZE_IN_PAGES_SIZE, FIRST_FREELIST_TRUNK_PAGE_OFFSET,
    FIRST_FREELIST_TRUNK_PAGE_SIZE, TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET,
    TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE,
};
use crate::errors::SqliteError;
use crate::storage::freelist::FreeList;
use crate::storage::page::{FIRST_FREEBLOCK_OFFSET, FIRST_FREEBLOCK_SIZE};

use super::buffer_pool::BufferPool;
use super::frame::FrameId;
use super::frame::{CLEAN, DIRTY, REFERENCED};
use super::guard::{BorrowState, PageGuard};
use super::journal::Journal;
use super::metadata::SqliteMetadata;
use super::raw_journal::{JournalMeta, RawJournal, RecoverMetadata};
use super::statistics::SqliteStatistics;
use crate::pager::frame::Frame;
use crate::vfs::file::SqliteFile;
use crate::{DbError, SqliteResult};

pub type PageNo = u32;

pub struct Pager<F: SqliteFile> {
    pub source: F,
    pub buffer_pool: BufferPool,
    journal: Journal,
    journal_pages: HashSet<PageNo>,
    // dirty pages linked list instead of new allocations
    pub dp_ll: Option<FrameId>,
    pub metadata: SqliteMetadata,
    pub statistics: SqliteStatistics,
    in_transaction: bool,
    /// Header fields at transaction start.
    /// Rollback reverts page images
    /// (including page 1) but not this struct, without the snapshot the
    /// next transaction would allocate/free using stale freelist state.
    /*
     * ISSUE: https://github.com/hamzader1/inkdb/issues/35
     */
    txn_snapshot: Option<SqliteMetadata>,
}

impl<F: SqliteFile> Pager<F> {
    pub fn new(
        source: F,
        page_size: usize,
        usable_size: usize,
        max_allocated_pages: usize,
        first_freelist_truck_page: u32,
        total_freelist_pages: u32,
    ) -> Result<Self, SqliteError> {
        let journal_meta = JournalMeta {
            db_name: source.name().to_string(),
            path: source.path(),
            p_size: page_size as _,
            db_size: source.len().expect("Error while trying to get the db len") as _,
        };
        let mut pager = Pager {
            source,
            buffer_pool: BufferPool::new(page_size),
            dp_ll: None,
            journal: Journal::uninit(),
            journal_pages: HashSet::new(),
            metadata: SqliteMetadata::new(
                page_size,
                usable_size,
                max_allocated_pages,
                first_freelist_truck_page,
                total_freelist_pages,
            ),
            statistics: SqliteStatistics::default(),
            in_transaction: false,
            txn_snapshot: None,
        };

        pager.recover_from_crash()?;
        pager.journal = Journal::new(journal_meta);
        Ok(pager)
    }

    pub fn with_cache(
        source: F,
        page_size: usize,
        usable_size: usize,
        max_allocated_pages: usize,
        first_freelist_truck_page: u32,
        total_freelist_pages: u32,

        cache_size: usize,
    ) -> Result<Self, SqliteError> {
        let journal_meta = JournalMeta {
            db_name: source.name().to_string(),
            path: source.path(),
            p_size: page_size as _,
            db_size: source.len().expect("Error while trying to get the db len") as _,
        };
        let mut pager = Pager {
            source,
            buffer_pool: BufferPool::with_cache(cache_size, page_size),
            dp_ll: None,
            journal: Journal::uninit(),
            journal_pages: HashSet::new(),
            metadata: SqliteMetadata::new(
                page_size,
                usable_size,
                max_allocated_pages,
                first_freelist_truck_page,
                total_freelist_pages,
            ),
            statistics: SqliteStatistics::default(),
            in_transaction: false,
            txn_snapshot: None,
        };

        pager.recover_from_crash()?;
        pager.journal = Journal::new(journal_meta);
        Ok(pager)
    }

    pub fn in_transaction(&self) -> bool {
        self.in_transaction
    }
    /// We do not start transaction immediately until
    /// we get a comfirmation by calling [`Pager::get_mut(..)`]
    pub fn start_transaction(&mut self) -> bool {
        if self.in_transaction {
            return false;
        }
        // Snapshot header state: rollback reverts page images but cannot
        // rewind this struct restore points come from here.
        self.txn_snapshot = Some(self.metadata);
        self.in_transaction = true;
        true
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
            .get_fast(page_no)
            .expect("page should be present after loading it"))
    }

    pub fn get_mut(&mut self, page_no: PageNo) -> Result<PageGuard, DbError> {
        Self::validate_page(
            page_no,
            self.metadata.max_allocated_pages,
            None::<fn(_) -> bool>,
        )?;
        let was_dirty = self.ensure_page_loaded(page_no)?;
        // Init once per transaction: re-opening on every mut touch
        // truncates the journal file back to its header each time.
        if self.journal.is_active() && !self.journal.is_init() {
            self.journal.init()?;
        }
        Ok(self
            .get_fast_mut(page_no, was_dirty)?
            .expect("page should be present after loading it"))
    }

    // cache look up
    fn get_fast(&mut self, page_no: PageNo) -> Option<PageGuard> {
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

    fn get_fast_mut(
        &mut self,
        page_no: PageNo,
        was_dirty: bool,
    ) -> Result<Option<PageGuard>, SqliteError> {
        if let Some(frame_id) = self.buffer_pool.page_table.get(&page_no) {
            debug_assert!(
                self.in_transaction,
                "Cannot mutably access a page outside of a transaction"
            );
            let frame_id = *frame_id;
            let frame = &mut self.buffer_pool.frame_buffer[frame_id];
            frame.incr_pin_count();
            frame.clear(CLEAN);
            frame.set(REFERENCED | DIRTY);

            /*
             *
             * This fixes the bug of inserting the same node twice
             * for example in call like
             * let p2_rc = get_mut(page_2);
             * let p2_rc_2 = get_mut(page_2);
             * this will insert the page twice
             * which can also cause infinite loop (pointer point to it self)
             *
             */
            if !was_dirty {
                self.dp_ll_insert(frame_id);
            }
            let mut page_guard = self.page_guard_mut(frame_id);
            if self.journal.is_active() && !self.journal_pages.contains(&page_no) {
                self.journal_pages.insert(page_no);
                self.journal
                    .add_page(page_no, page_guard.bytes_as_mut().unwrap());
            }
            return Ok(Some(page_guard));
        }
        Ok(None)
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
            self.source.sync()?;
            self.journal_pages.remove(&frame_page_no);
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
        let is_tail = frame_id == self.dp_ll.unwrap(); // if this panics, we have a bug

        let frame = &mut self.buffer_pool.frame_buffer[frame_id];
        next = frame.next;
        prev = frame.prev;
        // in case this returned the buffer pool,
        // should not handle its old pointers so it breaks the list
        frame.prev = None;
        frame.next = None;

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
            return Err(SqliteError::Internal(format!(
                "page guard exception rejected page {page_no}"
            )));
        }
        if page_no == 0 || page_no as usize > max_pages {
            return Err(SqliteError::InvalidPageNumber(page_no));
        }

        Ok(())
    }
    pub fn cached_page_count(&self) -> usize {
        self.buffer_pool.frame_buffer.len() - self.buffer_pool.free_frames.len()
    }

    pub fn commit(&mut self) -> Result<(), SqliteError> {
        if self.journal.is_init() {
            self.journal.commit()?;
            self.flush_all()?;
            self.source.sync()?;
            self.journal.destroy_internal()?;
        }
        // Drop per transaction state: without this the next transaction
        // replays (or rolls back) pages from already committed ones.
        if self.journal.is_active() {
            self.journal.reset();
        }
        self.journal_pages.clear();
        // Committed header state stands.
        // The snapshot has served.
        self.txn_snapshot = None;
        self.in_transaction = false;

        Ok(())
    }

    pub fn rollback(&mut self) -> Result<(), SqliteError> {
        if self.journal.is_init() {
            let mut iterator = self.journal.make_iterator();
            while let Some(page) = iterator.iter()? {
                if self.journal_pages.contains(&page.page_no) {
                    let mut page_guard = self.get_mut(page.page_no)?;
                    page_guard
                        .bytes_as_mut_unchecked()
                        .copy_from_slice(page.data);
                }
            }
            self.journal.destroy_internal()?;
        }
        if self.journal.is_active() {
            self.journal.reset();
        }
        self.journal_pages.clear();
        // Replay restored page images (including page 1).
        // Now rewind the in memory header to match. Done after replay: pages allocated
        // mid-transaction still validate while being revisited above.
        if let Some(snapshot) = self.txn_snapshot.take() {
            self.metadata = snapshot;
        }
        self.in_transaction = false;
        Ok(())
    }

    // TODO: DO NOT USE RAWJOURNAL. JOURNAL INSTEAD.
    pub fn recover_from_crash(&mut self) -> Result<(), SqliteError> {
        let recover_meta = RawJournal::recover(self.source.name(), self.source.path())?;
        if let Some(meta) = recover_meta {
            let RecoverMetadata {
                mut iterator,
                db_size,
                file_path,
            } = meta;
            while let Some(page) = iterator.iter()? {
                if self.get_page_offset(page.page_no) < db_size {
                    let mut page_guard = self.get_mut(page.page_no)?;
                    page_guard
                        .bytes_as_mut()
                        .unwrap()
                        .copy_from_slice(page.data);
                }
            }
            self.flush_all()?;
            self.source.set_len(db_size)?;
            self.source.sync()?;
            RawJournal::destroy_external(file_path)?;
        }
        Ok(())
    }

    pub fn allocate_new_page(&mut self) -> Result<PageNo, SqliteError> {
        // Freelist check
        let first_freelist_truck_page = self.metadata.first_freelist_truck_page;
        let total_free_pages = self.metadata.total_freelist_pages;
        // Freelist as 1st source
        if let Some(alloc_meta) =
            FreeList::new(self).alloc(first_freelist_truck_page, total_free_pages)?
        {
            self.metadata.first_freelist_truck_page = alloc_meta.first_freelist_trunk_page;
            self.metadata.total_freelist_pages = alloc_meta.total_freelist_pages;
            self.update_first_freelist_truck_page()?;
            self.update_total_free_pages()?;
            return Ok(alloc_meta.allocated_page.unwrap());
        }
        let max_allocated_pages = self.metadata.max_allocated_pages;
        let new_page_no = max_allocated_pages + 1;
        let new_len = self.metadata.page_size * (max_allocated_pages + 1);
        self.source.set_len(new_len)?;
        self.metadata.max_allocated_pages += 1;
        self.update_max_allocated_pages()?;
        Ok(new_page_no as _)
    }
    pub fn update_max_allocated_pages(&mut self) -> Result<(), SqliteError> {
        let mut guard = self.get_mut(1)?;
        let bytes = guard.bytes_as_mut_unchecked();
        bytes[DATABASE_SIZE_IN_PAGES_OFFSET
            ..DATABASE_SIZE_IN_PAGES_OFFSET + DATABASE_SIZE_IN_PAGES_SIZE]
            .copy_from_slice(&(self.metadata.max_allocated_pages as u32).to_be_bytes());

        Ok(())
    }

    pub fn dealloc(&mut self, page_no: PageNo) -> SqliteResult<()> {
        let first_freelist_truck_page = self.metadata.first_freelist_truck_page;
        let total_free_pages = self.metadata.total_freelist_pages;
        let usable_size = self.metadata.usable_size;

        let dealloc_meta = FreeList::new(self).push(
            page_no,
            first_freelist_truck_page,
            total_free_pages,
            usable_size,
        )?;
        if dealloc_meta.first_freelist_trunk_page != first_freelist_truck_page {
            self.metadata.first_freelist_truck_page = dealloc_meta.first_freelist_trunk_page;
            self.update_first_freelist_truck_page()?;
        }
        if dealloc_meta.total_freelist_pages != total_free_pages {
            self.metadata.total_freelist_pages = dealloc_meta.total_freelist_pages;
            self.update_total_free_pages()?;
        }
        Ok(())
    }

    pub fn update_first_freelist_truck_page(&mut self) -> Result<(), SqliteError> {
        let mut guard = self.get_mut(1)?;
        let bytes = guard.bytes_as_mut_unchecked();
        bytes[FIRST_FREELIST_TRUNK_PAGE_OFFSET
            ..FIRST_FREELIST_TRUNK_PAGE_OFFSET + FIRST_FREELIST_TRUNK_PAGE_SIZE]
            .copy_from_slice(&(self.metadata.first_freelist_truck_page).to_be_bytes());

        Ok(())
    }
    pub fn update_total_free_pages(&mut self) -> SqliteResult<()> {
        let mut guard = self.get_mut(1)?;
        let bytes = guard.bytes_as_mut_unchecked();
        bytes[TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET
            ..TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET + TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE]
            .copy_from_slice(&(self.metadata.total_freelist_pages).to_be_bytes());

        Ok(())
    }
}

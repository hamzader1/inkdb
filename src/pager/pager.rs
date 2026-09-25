use std::collections::HashSet;
use std::ptr::NonNull;

use crate::db::header::{
    DATABASE_SIZE_IN_PAGES_OFFSET, DATABASE_SIZE_IN_PAGES_SIZE, FIRST_FREELIST_TRUNK_PAGE_OFFSET,
    FIRST_FREELIST_TRUNK_PAGE_SIZE, SqliteDatabaseHeader, TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET,
    TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE,
};
use crate::errors::SqliteError;
use crate::util::sqlite_assert_with_runtime_err;

use super::buffer_pool::{Acquire, BufferPool};
use super::frame::FrameId;
use super::guard::{BorrowState, PageGuard};
use super::journal::Journal;
use super::raw_journal::{JournalMeta, RawJournal, RecoverMetadata};
use super::statistics::Statistics;
use crate::vfs::Vfs;
use crate::vfs::file::SqliteFile;
use crate::{DbError, SqliteCursor, SqliteResult};

pub type PageNo = u32;

pub struct Pager<V: Vfs> {
    vfs: V,
    source: V::File,
    buffer_pool: BufferPool,
    journal: Journal<V::File>,
    journal_pages: HashSet<PageNo>,
    pub header: HeaderCache,
    flushed: HashSet<PageNo>,
    statistics: Statistics,
    in_transaction: bool,
    txn_snapshot: Option<HeaderCache>,
}

#[derive(Debug, Clone, Copy)]
pub struct HeaderCache {
    page_size: u32,
    usable_size: u32,
    max_allocated_pages: u32,
    first_freelist_truck_page: u32,
    total_freelist_pages: u32,
}

impl HeaderCache {
    pub fn new(
        page_size: u32,
        usable_size: u32,
        max_allocated_pages: u32,
        first_freelist_truck_page: u32,
        total_freelist_pages: u32,
    ) -> Self {
        Self {
            page_size,
            usable_size,
            max_allocated_pages,
            first_freelist_truck_page,
            total_freelist_pages,
        }
    }
}

impl From<SqliteDatabaseHeader> for HeaderCache {
    fn from(value: SqliteDatabaseHeader) -> Self {
        HeaderCache::new(
            value.database_page_size,
            value.database_page_size - value.reserved_space as u32,
            value.database_size_in_pages,
            value.first_freelist_trunk_page,
            value.total_number_of_freelist_pages,
        )
    }
}

impl<V: Vfs> Pager<V> {
    pub fn new(vfs: V, source: V::File, header: HeaderCache) -> Result<Self, SqliteError> {
        let journal_meta = JournalMeta {
            db_size: source.len().expect("len") as _,
            p_size: header.page_size as _,
        };
        let mut pager = Pager {
            vfs,
            source,
            buffer_pool: BufferPool::new(header.page_size as _),
            journal: Journal::Disabled,
            flushed: HashSet::new(),
            journal_pages: HashSet::new(),
            header,
            statistics: Statistics::default(),
            in_transaction: false,
            txn_snapshot: None,
        };
        pager.recover_from_crash()?;
        pager.journal.enable(journal_meta);
        Ok(pager)
    }
    pub fn with_cache(
        vfs: V,
        source: V::File,
        header: HeaderCache,
        cache_size: usize,
    ) -> Result<Self, SqliteError> {
        let journal_meta = JournalMeta {
            db_size: source.len().expect("len") as _,
            p_size: header.page_size as _,
        };
        let mut pager = Pager {
            vfs,
            source,
            buffer_pool: BufferPool::with_cache(cache_size, header.page_size as _),
            journal: Journal::Disabled,
            journal_pages: HashSet::new(),
            flushed: HashSet::new(),
            header,
            statistics: Statistics::default(),
            in_transaction: false,
            txn_snapshot: None,
        };
        pager.recover_from_crash()?;
        pager.journal.enable(journal_meta);
        Ok(pager)
    }
    pub fn in_transaction(&self) -> bool {
        self.in_transaction
    }
    pub fn start_transaction(&mut self) -> bool {
        if self.in_transaction {
            return false;
        }
        self.txn_snapshot = Some(self.header);
        self.in_transaction = true;
        true
    }
    pub fn validate_page(page_no: PageNo, max_pages: u32) -> Result<(), DbError> {
        if page_no == 0 || page_no > max_pages {
            return Err(SqliteError::InvalidPageNumber(page_no));
        }
        Ok(())
    }
    pub fn cached_page_count(&self) -> usize {
        self.buffer_pool.cached_count()
    }
    pub fn page_size(&self) -> usize {
        self.header.page_size as _
    }
    pub fn usable_size(&self) -> usize {
        self.header.usable_size as _
    }
    pub fn max_allocation_pages(&self) -> usize {
        self.header.max_allocated_pages as _
    }
    fn get_page_offset(&self, page_no: PageNo) -> usize {
        ((page_no as usize) - 1) * self.header.page_size as usize
    }
    fn guard(&mut self, id: FrameId, state: BorrowState) -> PageGuard {
        let pool = self.buffer_pool.as_ptr_mut();
        let len = self.header.page_size as usize;
        let ptr =
            unsafe { NonNull::new_unchecked(self.buffer_pool.frame_bytes_mut(id).as_mut_ptr()) };
        let slice = NonNull::<[u8]>::slice_from_raw_parts(ptr, len);
        PageGuard::new(pool, id, slice, state)
    }

    // Todo: temporary turn off for the borrow guard
    pub fn get(&mut self, page_no: PageNo) -> SqliteResult<PageGuard> {
        Self::validate_page(page_no, self.header.max_allocated_pages)?;
        match self.buffer_pool.acquire(page_no)? {
            Acquire::Hit(frameid) => {
                // self.buffer_pool.borrow(frameid, page_no)?;
                self.statistics.inc_cache_hit();
                Ok(self.guard(frameid, BorrowState::Ref))
            }
            Acquire::Miss { frameid, evicted } => {
                // self.buffer_pool.borrow(frameid, page_no)?;
                if let Some(ev) = evicted {
                    self.statistics.inc_evictions();
                    if ev.was_dirty {
                        sqlite_assert_with_runtime_err(
                            matches!(self.journal, Journal::Open { .. }),
                            || {
                                format!(
                                    "steal of dirty page {} with journal not open: persist must precede db flush",
                                    ev.page_no
                                )
                            },
                        )?;
                        self.journal.persist_tail()?;
                        self.flush_page(ev.page_no, frameid)?;
                        self.flushed.insert(ev.page_no);
                    }
                }
                let offset = self.get_page_offset(page_no);
                self.source
                    .read_exact_at(offset as _, self.buffer_pool.frame_bytes_mut(frameid))?;
                self.statistics.inc_cache_miss();
                Ok(self.guard(frameid, BorrowState::Ref))
            }
        }
    }
    // Todo: temporary turn off for the borrow guard
    pub fn get_mut(&mut self, page_no: PageNo) -> SqliteResult<PageGuard> {
        Self::validate_page(page_no, self.header.max_allocated_pages)?;
        debug_assert!(self.in_transaction, "get mut forbidden outside of txn");
        let frameid = match self.buffer_pool.acquire(page_no)? {
            Acquire::Hit(frameid) => {
                // self.buffer_pool.exclusive_borrow(frameid, page_no)?;

                self.statistics.inc_cache_hit();
                frameid
            }
            Acquire::Miss { frameid, evicted } => {
                // self.buffer_pool.exclusive_borrow(frameid, page_no)?;
                if let Some(ev) = evicted {
                    self.statistics.inc_evictions();
                    if ev.was_dirty {
                        sqlite_assert_with_runtime_err(
                            matches!(self.journal, Journal::Open { .. }),
                            || {
                                format!(
                                    "steal of dirty page {} with journal not open: persist must precede db flush",
                                    ev.page_no
                                )
                            },
                        )?;
                        self.journal.persist_tail()?;
                        self.flush_page(ev.page_no, frameid)?;
                        self.flushed.insert(ev.page_no);
                    }
                }
                let offset = self.get_page_offset(page_no);
                self.source
                    .read_exact_at(offset as _, self.buffer_pool.frame_bytes_mut(frameid))?;
                self.statistics.inc_cache_miss();

                frameid
            }
        };
        if !matches!(self.journal, Journal::Disabled) && !self.journal_pages.contains(&page_no) {
            if matches!(self.journal, Journal::Idle(_)) {
                let file = self.vfs.open_journal(&self.source)?;
                self.journal.init(file)?;
            }
            self.journal_pages.insert(page_no);
            if let Journal::Open { raw, .. } = &mut self.journal {
                raw.add_page(page_no, self.buffer_pool.frame_bytes(frameid));
            }
        }
        self.buffer_pool.mark_dirty(frameid);
        Ok(self.guard(frameid, BorrowState::RefMut))
    }
    fn flush_page(&mut self, page_no: PageNo, frameid: FrameId) -> Result<(), DbError> {
        let offset = self.get_page_offset(page_no);
        self.source
            .write_all_at(offset as _, self.buffer_pool.frame_bytes(frameid))?;
        self.buffer_pool.mark_clean(frameid);
        self.statistics.inc_disk_write();
        Ok(())
    }
    pub fn flush_all(&mut self) -> SqliteResult<()> {
        while let Some((page_no, frameid)) = self.buffer_pool.pop_dirty() {
            self.flush_page(page_no, frameid)?;
        }
        Ok(())
    }
    pub fn commit(&mut self) -> Result<(), SqliteError> {
        if let Journal::Open { raw, file, .. } = &mut self.journal {
            raw.commit(file)?;
            self.flush_all()?;
            self.source.sync()?;
            self.vfs.delete_journal(&self.source)?;
            self.journal.into_idle();
        }

        self.journal_pages.clear();
        self.flushed.clear();
        self.txn_snapshot = None;
        self.in_transaction = false;
        Ok(())
    }
    pub fn rollback(&mut self) -> Result<(), SqliteError> {
        if let Journal::Open { raw, .. } = &mut self.journal {
            let db_size = raw.db_size;
            let mut iterator = raw.make_iterator();
            while let Some(page) = iterator.iter()? {
                if self.journal_pages.contains(&page.page_no) {
                    match self.buffer_pool.lookup(page.page_no) {
                        Some(frame_id) => {
                            self.buffer_pool.restore_bytes(frame_id, page.data);
                            let was_written_to_disk = self.flushed.contains(&page.page_no);
                            if was_written_to_disk {
                                self.flush_page(page.page_no, frame_id)?;
                                self.source.sync()?;
                                self.flushed.remove(&page.page_no);
                            }
                            self.buffer_pool.mark_clean(frame_id);
                        }
                        _ => {
                            self.source
                                .write_all_at(self.get_page_offset(page.page_no) as _, page.data)?;
                        }
                    }
                }
            }
            while let Some((page_no, frameid)) = self.buffer_pool.pop_dirty() {
                let _ = (page_no, frameid);
            }
            self.source.set_len(db_size as usize)?;
            self.vfs.delete_journal(&self.source)?;
            self.journal.into_idle();
        }
        self.journal_pages.clear();
        self.flushed.clear();
        if let Some(snapshot) = self.txn_snapshot.take() {
            self.header = snapshot;
        }
        self.in_transaction = false;
        Ok(())
    }
    pub fn recover_from_crash(&mut self) -> Result<(), SqliteError> {
        let Some(bytes) = self.vfs.read_journal(&self.source)? else {
            return Ok(());
        };
        let Some(meta) = RawJournal::parse_recovery(bytes)? else {
            self.vfs.delete_journal(&self.source)?;
            return Ok(());
        };
        let RecoverMetadata {
            mut iterator,
            db_size,
        } = meta;
        while let Some(page) = iterator.iter()? {
            if self.get_page_offset(page.page_no) < db_size {
                let mut guard = self.get_mut(page.page_no)?;
                guard.bytes_as_mut_unchecked().copy_from_slice(page.data);
            }
        }
        self.flush_all()?;
        self.source.set_len(db_size)?;
        self.source.sync()?;
        self.vfs.delete_journal(&self.source)?;
        self.journal_pages.clear();
        self.flushed.clear();
        Ok(())
    }
    pub fn allocate_new_page(&mut self) -> Result<PageNo, SqliteError> {
        let first = self.header.first_freelist_truck_page;
        let total = self.header.total_freelist_pages;
        if let Some((allocated, next_head, next_total)) = self.freelist_alloc(first, total)? {
            self.header.first_freelist_truck_page = next_head;
            self.header.total_freelist_pages = next_total;
            self.update_first_freelist_truck_page()?;
            self.update_total_free_pages()?;
            return Ok(allocated);
        }
        let max_allocated_pages = self.header.max_allocated_pages;
        let new_page_no = max_allocated_pages + 1;
        let new_len = self.header.page_size * (max_allocated_pages + 1);
        self.source.set_len(new_len as _)?;
        self.header.max_allocated_pages += 1;
        self.update_max_allocated_pages()?;
        Ok(new_page_no as _)
    }
    fn freelist_alloc(
        &mut self,
        first: u32,
        total: u32,
    ) -> SqliteResult<Option<(PageNo, u32, u32)>> {
        match (first, total) {
            (0, 0) => return Ok(None),
            (0, _) => {
                return Err(SqliteError::Corrupt(
                    "freelist count is nonzero but first trunk page is zero".into(),
                ));
            }
            (_, 0) => {
                return Err(SqliteError::Corrupt(
                    "freelist trunk page is nonzero but freelist count is zero".into(),
                ));
            }
            _ => {}
        };
        if first == 1 {
            return Err(SqliteError::Corrupt("trunk page is page 1".into()));
        }
        let mut guard = self.get_mut(first)?;
        let bytes = guard.bytes_as_mut_unchecked();
        let mut cursor = SqliteCursor::new(bytes);
        let next_page_no = cursor.read_next_u32()?;
        let leaf_count = cursor.read_next_u32()?;
        if leaf_count == 0 {
            return Ok(Some((first, next_page_no, total - 1)));
        }
        cursor.move_forward_by((4 * (leaf_count - 1)) as _)?;
        let last_leaf = cursor.read_next_u32()?;
        if last_leaf == 1 {
            return Err(SqliteError::Corrupt("leaf page is page 1".into()));
        }
        bytes[4..8].copy_from_slice(&u32::to_be_bytes(leaf_count - 1));
        Ok(Some((last_leaf, first, total - 1)))
    }
    pub fn dealloc(&mut self, page_no: PageNo) -> SqliteResult<()> {
        let first = self.header.first_freelist_truck_page;
        let total = self.header.total_freelist_pages;
        let usable_size = self.header.usable_size;
        let (next_head, next_total) =
            self.freelist_push(page_no, first, total, usable_size as _)?;
        if next_head != first {
            self.header.first_freelist_truck_page = next_head;
            self.update_first_freelist_truck_page()?;
        }
        if next_total != total {
            self.header.total_freelist_pages = next_total;
            self.update_total_free_pages()?;
        }
        Ok(())
    }
    fn freelist_push(
        &mut self,
        page_no: PageNo,
        first: u32,
        total: u32,
        usable_size: usize,
    ) -> SqliteResult<(u32, u32)> {
        let mut current = first;
        while current != 0 {
            let next_page_no;
            let leaf_count;
            {
                let mut guard = self.get_mut(current)?;
                let bytes = guard.bytes_as_mut_unchecked();
                let mut cursor = SqliteCursor::new(bytes);
                next_page_no = cursor.read_next_u32()?;
                leaf_count = cursor.read_next_u32()?;
                let leaf_offset = 8usize + 4usize * leaf_count as usize;
                if leaf_offset + 4 <= usable_size {
                    cursor.move_forward_by(u64::from(leaf_count * 4))?;
                    let curr_pos = cursor.stream_pos() as usize;
                    bytes[curr_pos..curr_pos + 4].copy_from_slice(&u32::to_be_bytes(page_no));
                    bytes[4..8].copy_from_slice(&u32::to_be_bytes(leaf_count + 1));
                    return Ok((first, total + 1));
                }
            }
            current = next_page_no;
        }
        {
            let mut guard = self.get_mut(page_no)?;
            let bytes = guard.bytes_as_mut_unchecked();
            bytes[0..4].copy_from_slice(&u32::to_be_bytes(first));
            bytes[4..8].copy_from_slice(&[0, 0, 0, 0]);
        }
        Ok((page_no, total + 1))
    }
    pub fn update_max_allocated_pages(&mut self) -> Result<(), SqliteError> {
        let mut guard = self.get_mut(1)?;
        let bytes = guard.bytes_as_mut_unchecked();
        bytes[DATABASE_SIZE_IN_PAGES_OFFSET
            ..DATABASE_SIZE_IN_PAGES_OFFSET + DATABASE_SIZE_IN_PAGES_SIZE]
            .copy_from_slice(&(self.header.max_allocated_pages).to_be_bytes());
        Ok(())
    }
    pub fn update_first_freelist_truck_page(&mut self) -> Result<(), SqliteError> {
        let mut guard = self.get_mut(1)?;
        let bytes = guard.bytes_as_mut_unchecked();
        bytes[FIRST_FREELIST_TRUNK_PAGE_OFFSET
            ..FIRST_FREELIST_TRUNK_PAGE_OFFSET + FIRST_FREELIST_TRUNK_PAGE_SIZE]
            .copy_from_slice(&(self.header.first_freelist_truck_page).to_be_bytes());
        Ok(())
    }
    pub fn update_total_free_pages(&mut self) -> SqliteResult<()> {
        let mut guard = self.get_mut(1)?;
        let bytes = guard.bytes_as_mut_unchecked();
        bytes[TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET
            ..TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET + TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE]
            .copy_from_slice(&(self.header.total_freelist_pages).to_be_bytes());
        Ok(())
    }
}

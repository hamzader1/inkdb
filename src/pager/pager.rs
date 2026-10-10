use std::collections::HashSet;
use std::ptr::NonNull;

use crate::db::header::{
    DatabaseHeader, DbFormat, InkFileHeader, SQLITE_DATABASE_SIZE_IN_PAGES_SIZE,
    SQLITE_FIRST_FREELIST_TRUNK_PAGE_SIZE, SQLITE_TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE,
    SqliteDatabaseHeader,
};
use crate::errors::InkError;
use crate::util::assert_with_runtime_err;

use super::buffer_pool::{Acquire, BufferPool};
use super::frame::FrameId;
use super::guard::{BorrowState, PageGuard};
use super::journal::Journal;
use super::raw_journal::{JournalMeta, RawJournal, RecoverMetadata};
use super::statistics::Statistics;
use crate::InkResult;
use crate::vfs::Vfs;
use crate::vfs::file::InkFile;

/// How many pages the cache holds when no other size is asked for.
const DEFAULT_CACHE_SIZE: usize = 4096;

/// The number of a page. Page one is the database header, so page numbers start
/// at one and zero is never a real page.
pub type PageNo = u32; /* Replace this with a `PageNo(usize)` new type */

/// Reads and writes the pages of one database file.
///
/// Everything that touches a page goes through the pager. It keeps the pages it
/// has read in the buffer pool, so a page that is used twice is only read once,
/// and it keeps a journal of the pages a write changes, so a crash partway
/// through can be put right. None of this is visible from outside: a page is
/// asked for, a guard comes back, the bytes are changed through the guard, and
/// the pager works out when they reach the file.
#[rustfmt::skip]
#[derive(Debug)]
pub struct Pager<V: Vfs> {
    vfs:               V,
    source:            V::File,
    buffer_pool:       BufferPool,
    journal:           Journal<V::File>,
    journal_pages:     HashSet<PageNo>,
    pub(crate) header: HeaderCache,
    flushed:           HashSet<PageNo>,
    statistics:        Statistics,
    in_transaction:    bool,
    txn_snapshot:      Option<HeaderCache>,
}

#[derive(Debug, Clone, Copy)]
pub struct HeaderCache {
    page_size: u32,
    usable_size: u32,
    max_allocated_pages: u32,
    pub(crate) first_freelist_truck_page: u32,
    pub(crate) total_freelist_pages: u32,
    header_len: usize,
    format: DbFormat,
}

impl HeaderCache {
    pub fn new(
        page_size: u32,
        usable_size: u32,
        max_allocated_pages: u32,
        first_freelist_truck_page: u32,
        total_freelist_pages: u32,
        header_len: usize,
        format: DbFormat,
    ) -> Self {
        Self {
            page_size,
            usable_size,
            max_allocated_pages,
            first_freelist_truck_page,
            total_freelist_pages,
            header_len,
            format,
        }
    }

    /// How many bytes the header takes on the first page, which is where the
    /// first page's content begins.
    pub fn header_len(&self) -> usize {
        self.header_len
    }

    /// Which layout the file uses, SQLite's or this database's own.
    pub fn format(&self) -> DbFormat {
        self.format
    }
}

/// Work the header values out from a SQLite file's header.
impl From<SqliteDatabaseHeader> for HeaderCache {
    fn from(value: SqliteDatabaseHeader) -> Self {
        HeaderCache::new(
            value.database_page_size(),
            value.database_page_size() - value.reserved_space() as u32,
            value.database_size_in_pages(),
            value.first_freelist_trunk_page(),
            value.total_number_of_freelist_pages(),
            DbFormat::Sqlite.header_len(),
            DbFormat::Sqlite,
        )
    }
}

/// Work the header values out from this database's own file header. A page size
/// written as one means the largest page a SQLite file allows, which is 65536
/// bytes, so it is turned back into that number here.
impl From<InkFileHeader> for HeaderCache {
    fn from(value: InkFileHeader) -> Self {
        let page_size = if value.database_page_size() == 1 {
            65536
        } else {
            value.database_page_size()
        };
        HeaderCache::new(
            page_size,
            page_size - value.reserved_space() as u32,
            value.database_size_in_pages(),
            value.first_freelist_trunk_page(),
            value.total_number_of_freelist_pages(),
            DbFormat::Ink.header_len(),
            DbFormat::Ink,
        )
    }
}

/// Work the header values out from whichever kind of header the file has.
impl From<DatabaseHeader> for HeaderCache {
    fn from(value: DatabaseHeader) -> Self {
        match value {
            DatabaseHeader::Sqlite(h) => HeaderCache::from(h),
            DatabaseHeader::Ink(h) => HeaderCache::from(h),
        }
    }
}

impl<V: Vfs> Pager<V> {
    pub fn new(vfs: V, source: V::File, header: HeaderCache) -> Result<Self, InkError> {
        Self::with_cache(vfs, source, header, DEFAULT_CACHE_SIZE)
    }
    pub fn with_cache(
        vfs: V,
        source: V::File,
        header: HeaderCache,
        cache_size: usize,
    ) -> Result<Self, InkError> {
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
    /// Whether a write is under way.
    pub fn in_transaction(&self) -> bool {
        self.in_transaction
    }

    /// Begin a write.
    ///
    /// The size the file has now is noted in the journal, since a rollback has
    /// to bring the file back to it, and a snapshot of the header is taken for
    /// the same reason. Answers whether a write actually began: one that is
    /// already running is left alone.
    pub fn start_transaction(&mut self) -> bool {
        if self.in_transaction {
            // Txn already started somewhere before
            return false;
        }
        if let Ok(len) = self.source.len() {
            self.journal.record_db_size(len as u32);
        }
        self.txn_snapshot = Some(self.header);
        self.in_transaction = true;
        // Txn just started
        true
    }
    /// Check that a page number is one the file could actually have.
    pub fn validate_page(page_no: PageNo, max_pages: u32) -> Result<(), InkError> {
        if page_no == 0 || page_no > max_pages {
            return Err(InkError::InvalidPageNumber(page_no));
        }
        Ok(())
    }
    /// How many pages are in the cache right now.
    pub fn cached_page_count(&self) -> usize {
        self.buffer_pool.cached_count()
    }
    /// How many bytes one page takes.
    pub fn page_size(&self) -> usize {
        self.header.page_size as _
    }
    /// How much of a page can hold data, with the reserved space left out.
    pub fn usable_size(&self) -> usize {
        self.header.usable_size as _
    }
    /// The virtual file system the database uses.
    pub(crate) fn vfs_mut(&mut self) -> &mut V {
        &mut self.vfs
    }
    /// How many bytes the header takes on the first page.
    pub fn header_len(&self) -> usize {
        self.header.header_len()
    }

    /// Which layout the file uses.
    pub fn format(&self) -> DbFormat {
        self.header.format()
    }

    /// How many pages the file holds.
    pub fn max_allocation_pages(&self) -> usize {
        self.header.max_allocated_pages as _
    }

    /// Where a page begins in the file. Page numbers start at one, so the first
    /// page begins at the start of the file.
    fn get_page_offset(&self, page_no: PageNo) -> usize {
        ((page_no as usize) - 1) * self.header.page_size as usize
    }
    /// Wrap a frame in a guard, pointing it at the frame's bytes.
    fn guard(&mut self, id: FrameId, state: BorrowState) -> PageGuard {
        let pool = self.buffer_pool.as_ptr_mut();
        let len = self.header.page_size as usize;
        let ptr =
            unsafe { NonNull::new_unchecked(self.buffer_pool.frame_bytes_mut(id).as_mut_ptr()) };
        let slice = NonNull::<[u8]>::slice_from_raw_parts(ptr, len);
        PageGuard::new(pool, id, slice, state)
    }

    /// Read a page, without the right to change it.
    ///
    /// The page comes from the cache when it is already there and from the file
    /// when it is not. A frame that has to be handed back for the new page is
    /// written first if it had changes in it, so nothing is lost by taking its
    /// place.
    pub fn get(&mut self, page_no: PageNo) -> InkResult<PageGuard> {
        Self::validate_page(page_no, self.header.max_allocated_pages)?;
        match self.buffer_pool.acquire(page_no)? {
            Acquire::Hit(frameid) => {
                self.statistics.inc_cache_hit();
                Ok(self.guard(frameid, BorrowState::Ref))
            }
            Acquire::Miss { frameid, evicted } => {
                if let Some(ev) = evicted {
                    self.statistics.inc_evictions();
                    // If the frame was dirty, the journal must reach disk before the
                    // database page. Otherwise, the new page could reach disk without
                    // its old version being safely stored in the journal, making rollback
                    // impossible after a crash.
                    if ev.was_dirty {
                        assert_with_runtime_err(
                            matches!(self.journal, Journal::Open { .. }),
                            || {
                                format!(
                                    "steal of dirty page {} with journal not open: persist must precede db flush",
                                    ev.page_no
                                )
                            },
                        )?;
                        // Safely save pages to disk first.
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
    /// Read a page, with the right to change it.
    ///
    /// The page is recorded in the journal before it is handed out, since the
    /// we are about to change it and the journal has to hold the bytes it had
    /// before. A page is only recorded once, no matter how many times it is
    /// gotten this way.
    pub fn get_mut(&mut self, page_no: PageNo) -> InkResult<PageGuard> {
        Self::validate_page(page_no, self.header.max_allocated_pages)?;
        debug_assert!(self.in_transaction, "get mut forbidden outside of txn");
        let frameid = match self.buffer_pool.acquire(page_no)? {
            Acquire::Hit(frameid) => {
                /*
                 *  Enable this once the borrowing conflict is fixed.
                 *  self.buffer_pool.exclusive_borrow(frameid, page_no)?;
                 */

                self.statistics.inc_cache_hit();
                frameid
            }
            Acquire::Miss { frameid, evicted } => {
                /*
                 *  Enable this once the borrowing conflict is fixed.
                 *  self.buffer_pool.exclusive_borrow(frameid, page_no)?;
                 */
                if let Some(ev) = evicted {
                    self.statistics.inc_evictions();
                    // Read the same block above in Pager::get.
                    if ev.was_dirty {
                        assert_with_runtime_err(
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
    /// Write one page's bytes to the file and mark its frame as clean.
    fn flush_page(&mut self, page_no: PageNo, frameid: FrameId) -> Result<(), InkError> {
        let offset = self.get_page_offset(page_no);
        self.source
            .write_all_at(offset as _, self.buffer_pool.frame_bytes(frameid))?;
        self.buffer_pool.mark_clean(frameid);
        self.statistics.inc_disk_write();
        Ok(())
    }
    /// Write every page that has changes, oldest first, until none are left.
    pub fn flush_all(&mut self) -> InkResult<()> {
        while let Some((page_no, frameid)) = self.buffer_pool.pop_dirty() {
            self.flush_page(page_no, frameid)?;
        }
        Ok(())
    }
    /// Finish a write: commit the journal, write every changed page to the
    /// file, sync it, and remove the journal.
    ///
    /// Committing the journal first is what makes this safe. If a crash happens
    /// partway through writing the pages, the journal still holds every change
    /// and recovery can finish the job. When there is no open journal there is
    /// nothing to write, and this only resets the state that tracks the write.
    pub fn commit(&mut self) -> Result<(), InkError> {
        if let Journal::Open { raw, file, .. } = &mut self.journal {
            raw.commit(file)?;
            self.flush_all()?;
            self.source.sync()?;
            self.vfs.delete_journal(&self.source)?;
            self.journal.set_to_idle();
        }

        self.journal_pages.clear();
        self.flushed.clear();
        self.txn_snapshot = None;
        self.in_transaction = false;
        Ok(())
    }
    /// Undo a write: put every page the journal recorded back the way it was,
    /// bring the file back to the size it had, and remove the journal.
    ///
    /// A page is put right in whichever place it lives. If it is still in the
    /// cache its bytes are restored there, and if it was already written to the
    /// file the old bytes are written as well. If it is not in the cache at all,
    /// the old bytes go straight to the file. Finally the header is put back to
    /// the snapshot taken when the write began.
    pub fn rollback(&mut self) -> Result<(), InkError> {
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
            self.journal.set_to_idle();
        }
        self.journal_pages.clear();
        self.flushed.clear();
        if let Some(snapshot) = self.txn_snapshot.take() {
            self.header = snapshot;
        }
        self.in_transaction = false;
        Ok(())
    }
    /// Replay a journal that a crash left behind.
    ///
    /// When there is no journal, or the one there is was never committed, there
    /// is nothing to do and the journal is removed. Otherwise every page it
    /// holds is written back over the database, the file is trimmed to the size
    /// the journal says it should have, and the journal is removed.
    pub fn recover_from_crash(&mut self) -> Result<(), InkError> {
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
    /// Get a page for something new to be written into.
    ///
    /// A page off the freelist is used when there is one. Otherwise the file is
    /// grown by a page and the new page at the end is handed back.
    pub fn allocate_new_page(&mut self) -> Result<PageNo, InkError> {
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
    /// Write the number of pages the file holds into the header on page one.
    pub fn update_max_allocated_pages(&mut self) -> Result<(), InkError> {
        let mut guard = self.get_mut(1)?;
        let bytes = guard.bytes_as_mut_unchecked();
        let off = self.header.format().size_in_pages_offset();
        bytes[off..off + SQLITE_DATABASE_SIZE_IN_PAGES_SIZE]
            .copy_from_slice(&(self.header.max_allocated_pages).to_be_bytes());
        Ok(())
    }
    /// Write the first freelist trunk page into the header on page one.
    pub fn update_first_freelist_truck_page(&mut self) -> Result<(), InkError> {
        let mut guard = self.get_mut(1)?;
        let bytes = guard.bytes_as_mut_unchecked();
        let off = self.header.format().freelist_trunk_offset();
        bytes[off..off + SQLITE_FIRST_FREELIST_TRUNK_PAGE_SIZE]
            .copy_from_slice(&(self.header.first_freelist_truck_page).to_be_bytes());
        Ok(())
    }
    /// Write the number of pages on the freelist into the header on page one.
    pub fn update_total_free_pages(&mut self) -> InkResult<()> {
        let mut guard = self.get_mut(1)?;
        let bytes = guard.bytes_as_mut_unchecked();
        let off = self.header.format().freelist_total_offset();
        bytes[off..off + SQLITE_TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE]
            .copy_from_slice(&(self.header.total_freelist_pages).to_be_bytes());
        Ok(())
    }
}

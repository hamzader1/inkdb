use crate::errors::InkError;
use crate::vfs::file::InkFile;
use crate::{InkResult, MemCursor};

use super::pager::PageNo;

/// Initial journal capacity.
const JOURNAL_CAP: usize = 8;

/// Magic value identifying the journal.
const JOURNAL_MAGIC: u64 = 0x4A4F55524E414C31;

/// Offset of the journal magic.
#[allow(dead_code)]
const JOURNAL_MAGIC_OFFSET: usize = 0;

/// Offset of the page count.
pub const PAGE_COUNT_OFFSET: usize = 8;

/// Offset of the database size.
const DATABASE_SIZE_OFFSET: usize = 12;

/// Offset of the page size.
#[allow(dead_code)]
const PAGE_SIZE_OFFSET: usize = 16;

/// How many bytes the journal header takes before the first record.
pub(crate) const JOURNAL_HEADER_SIZE: usize = 20;

/// How many bytes a record spends on its page number, before the page itself.
const PAGE_NUMBER_SIZE: usize = 4;

/// The bytes of a journal, with a record for each page that has been changed.
///
/// A record is the page number followed by the page as it was before the
/// change. The header holds the magic, the number of records, the database size
/// and the page size.
///
/// ```text
///
///      8 bytes              4 bytes              4 bytes                  4 bytes
/// +----------------+--------------------+---------------------------+--------------------+
/// |  Magic number  | Number of records  |Intial database page count |     Page size      |
/// +----------------+--------------------+---------------------------+--------------------+
/// 0                8                    12                         16                    20
///
/// ```
///
#[derive(Default)]
pub(crate) struct RawJournal {
    /// The header and every record.
    pub(crate) buffer: Vec<u8>,
    /// How many bytes one page takes.
    pub(crate) page_size: u16,
    /// The size the database had when the journal was started.
    pub(crate) db_size: u32,
    /// How many records the journal holds.
    pub(crate) page_count: u32,
}

/// The two values a journal needs before it can be built.
pub(crate) struct JournalMeta {
    /// The size the database has at the moment.
    pub(crate) db_size: u32,
    /// How many bytes one page takes.
    pub(crate) p_size: u16,
}

impl RawJournal {
    pub fn new(JournalMeta { db_size, p_size }: JournalMeta) -> Self {
        let mut buffer: Vec<u8> = Vec::with_capacity(
            JOURNAL_HEADER_SIZE + ((PAGE_NUMBER_SIZE + p_size as usize) * JOURNAL_CAP),
        );
        buffer.extend_from_slice(&u64::to_be_bytes(JOURNAL_MAGIC));
        buffer.extend_from_slice(&u32::to_be_bytes(0));
        buffer.extend_from_slice(&u32::to_be_bytes(db_size));
        buffer.extend_from_slice(&u32::to_be_bytes(p_size as _));
        Self {
            buffer,
            page_count: 0,
            db_size,
            page_size: p_size,
        }
    }

    /// Change the database size in the header, which is what a rollback will
    /// bring the file back to.
    pub fn set_db_size(&mut self, db_size: u32) {
        self.db_size = db_size;
        self.buffer[DATABASE_SIZE_OFFSET..DATABASE_SIZE_OFFSET + 4]
            .copy_from_slice(&u32::to_be_bytes(db_size));
    }

    /// Give the journal file its size and write the header out.
    ///
    /// # Errors
    /// Whatever setting the length or writing reports.
    pub fn init<J: InkFile>(&mut self, file: &J) -> Result<(), InkError> {
        file.set_len(self.buffer.len())?;
        file.write_all_at(0, &self.buffer[0..JOURNAL_HEADER_SIZE])?;
        Ok(())
    }

    /// Add a record for a page: its number, then the page as it was before the
    /// change that is about to happen.
    /// ```text
    /// Structure of a log record.
    ///
    ///      4 bytes
    /// +----------------+----------------------------+
    /// |  Page Number   |    Database page image     |
    /// +----------------+----------------------------+
    ///
    /// ```
    pub fn add_page(&mut self, page_no: PageNo, data: &[u8]) {
        self.buffer.extend_from_slice(&u32::to_be_bytes(page_no));
        self.buffer.extend_from_slice(data);
        self.page_count += 1;
    }
    /// Commit the journal: write every record out with the record count set, so
    /// that a crash afterwards leaves a journal recovery can replay.
    ///
    /// The count is the commit mark, so it has to reach disk in the same write
    /// as the records it counts. Writing the records with a count of zero and
    /// fixing the count afterwards would leave a moment where a crash loses real
    /// records while pages they belong to have already been written to the
    /// database.
    ///
    /// # Errors
    /// Whatever setting the length, writing or syncing reports.
    pub fn commit<J: InkFile>(&mut self, file: &J) -> Result<(), InkError> {
        self.buffer[8..12].copy_from_slice(&u32::to_be_bytes(self.page_count));
        file.set_len(self.buffer.len())?;
        file.write_all_at(0 as _, &self.buffer)?;
        file.sync()?;
        Ok(())
    }

    /// A reader over the records this journal holds, for putting the pages
    /// back the way they were.
    pub fn make_iterator(&self) -> JournalIter {
        JournalIter::new(
            &self.buffer,
            self.page_count as _,
            (self.page_size + 4) as _,
        )
    }

    /// Drop every record and put the count back to zero, keeping the header.
    pub fn reset(&mut self) {
        self.buffer.truncate(JOURNAL_HEADER_SIZE);
        self.buffer[8..12].copy_from_slice(&[0, 0, 0, 0]);
        self.page_count = 0;
    }

    /// Read a journal file that was left behind and work out what recovery
    /// should do with it.
    ///
    /// Nothing is returned when the file does not start with the magic, or when
    /// its record count is zero. Either means there is no committed journal to
    /// replay, so there is nothing to recover.
    ///
    /// # Errors
    /// When the header is too short to hold its four fields.
    pub fn parse_recovery(bytes: Vec<u8>) -> Result<Option<RecoverMetadata>, InkError> {
        let mut cursor = MemCursor::new(&bytes);
        let magic = cursor.read_to(size_of::<u64>() as _)?;
        let page_count = cursor.read_next_u32()?;
        if page_count == 0 || magic != u64::to_be_bytes(JOURNAL_MAGIC) {
            return Ok(None);
        }
        let db_size = cursor.read_next_u32()?;
        let page_size = cursor.read_next_u32()?;
        let iterator = JournalIter::owned(
            bytes,
            page_count as _,
            (page_size as usize + PAGE_NUMBER_SIZE) as _,
        );
        let metadata = RecoverMetadata {
            iterator,
            db_size: db_size as _,
        };
        Ok(Some(metadata))
    }

    /// Write out the records from `start` on, along with the record count, so
    /// the file on disk matches the journal in memory up to the record written
    /// last.
    ///
    /// # Panics
    /// When `start` is past the end of the last record.
    ///
    /// # Errors
    /// Whatever setting the length, writing or syncing reports.
    pub fn persist_tail<J: InkFile>(&mut self, file: &J, start: usize) -> InkResult<()> {
        let end = JOURNAL_HEADER_SIZE + (self.page_count * (self.page_size as u32 + 4)) as usize;
        assert!(start <= end);
        if start == end {
            return Ok(());
        }
        let buff = &self.buffer[start..end];
        file.set_len(end)?;
        file.write_all_at(start as _, buff)?;
        file.write_all_at(PAGE_COUNT_OFFSET as _, &self.page_count.to_be_bytes())?;
        file.sync()?;
        Ok(())
    }
}
/// What recovery got out of a journal: the records to replay and the size the
/// database should have once they are.
pub(crate) struct RecoverMetadata {
    /// A reader over the journal's records.
    pub(crate) iterator: JournalIter,
    /// The size the database had when the journal was started.
    pub(crate) db_size: usize,
}

/// A reader over the records of a journal.
///
// It owns a copy of the journal's bytes, which keeps the reader simple at the
// cost of an allocation; borrowing them instead would be worth doing later.
// `hint` is how many records the journal says it has, and `step_by` is how many
// bytes one record takes, which is the page number plus the page.
#[rustfmt::skip]
pub(crate) struct JournalIter {
    bytes:  Vec<u8>,
    start:   usize,
    end:     usize,
    hint:    usize,
    count:   usize,
    step_by: usize,
}

/// One record of a journal: a page number and the page's old bytes.
pub(crate) struct JournalPage<'a> {
    /// The page this record is about.
    pub(crate) page_no: PageNo,
    /// The page as it was before the change.
    pub(crate) data: &'a [u8],
}
impl<'a> JournalPage<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        debug_assert!(
            bytes.len() >= 4,
            "Jounal page should at least hold the page number"
        );
        let page_no_bytes: [u8; 4] = *bytes[0..4].as_array().unwrap();
        let page_no = u32::from_be_bytes(page_no_bytes);
        Self {
            page_no,
            data: &bytes[4..],
        }
    }
}

impl JournalIter {
    pub fn new(bytes: &[u8], hint: usize, step_by: usize) -> Self {
        Self::owned(bytes.to_vec(), hint, step_by)
    }

    fn owned(bytes: Vec<u8>, hint: usize, step_by: usize) -> Self {
        Self {
            hint,
            bytes,
            start: JOURNAL_HEADER_SIZE,
            end: JOURNAL_HEADER_SIZE + step_by,
            step_by,
            count: 0,
        }
    }

    /// The next record, or nothing once every one the journal claims has been
    /// read.
    ///
    /// Reading the whole journal and calling this until it stops returns every
    /// record in the order they were added.
    ///
    /// # Errors
    /// When a record would run past the end of the journal's bytes.
    pub fn iter(&mut self) -> Result<Option<JournalPage<'_>>, InkError> {
        if self.hint == self.count {
            return Ok(None);
        }
        if self.end <= self.bytes.len() {
            let bytes = &self.bytes[self.start..self.end];
            self.end += self.step_by;
            self.start += self.step_by;
            self.count += 1;
            return Ok(Some(JournalPage::new(bytes)));
        }
        Ok(None)
    }
}
use std::fmt;

impl fmt::Debug for RawJournal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Journal")
            .field("buffer", &format_args!("<{} bytes>", self.buffer.len()))
            .field("page_size", &self.page_size)
            .field("page_count", &self.page_count)
            .finish()
    }
}

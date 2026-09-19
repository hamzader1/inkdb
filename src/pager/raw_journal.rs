use crate::errors::SqliteError;
use crate::vfs::file::SqliteFile;
use crate::{SqliteCursor, SqliteResult, size_of};

use super::pager::PageNo;

const JOURNAL_CAP: usize = 8;
// 1: 0..8
const JOURNAL_MAGIC: u64 = 0x4A4F55524E414C31;
const JOURNAL_MAGIC_OFFSET: usize = 0;
// 2: 8..12
pub const PAGE_COUNT_OFFSET: usize = 8;
// 3: 12..16
const DATABASE_SIZE_OFFSET: usize = 12;

// 4: 16..20
const PAGE_SIZE_OFFSET: usize = 16;

pub(crate) const JOURNAL_HEADER_SIZE: usize = 20;

const PAGE_NUMBER_SIZE: usize = 4;

#[derive(Default)]
pub struct RawJournal {
    pub buffer: Vec<u8>,
    pub page_size: u16,
    pub db_size: u32,
    pub page_count: u32,
}

pub struct JournalMeta {
    pub db_size: u32,
    pub p_size: u16,
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

    pub fn init<J: SqliteFile>(&mut self, file: &J) -> Result<(), SqliteError> {
        file.set_len(self.buffer.len())?;
        file.write_all_at(0, &self.buffer[0..JOURNAL_HEADER_SIZE])?;
        Ok(())
    }

    pub fn add_page(&mut self, page_no: PageNo, data: &[u8]) {
        self.buffer.extend_from_slice(&u32::to_be_bytes(page_no));
        self.buffer.extend_from_slice(data);
        self.page_count += 1;
    }
    pub fn commit<J: SqliteFile>(&mut self, file: &J) -> Result<(), SqliteError> {
        // The page count IS the commit record: it must be durable in the
        // same write as the data. Writing data first with count 0 and
        // patching after leaves a crash window where recovery discards
        // real records while evicted dirty pages are already on disk.
        self.buffer[8..12].copy_from_slice(&u32::to_be_bytes(self.page_count));
        // let file = self.jfile.as_mut().unwrap();
        file.set_len(self.buffer.len())?;
        file.write_all_at(0 as _, &self.buffer)?;
        file.sync()?;
        Ok(())
    }

    pub fn make_iterator(&self) -> JournalIter {
        JournalIter::new(
            &self.buffer,
            self.page_count as _,
            (self.page_size + 4) as _,
        )
    }

    pub fn reset(&mut self) {
        self.buffer.truncate(JOURNAL_HEADER_SIZE);
        self.buffer[8..12].copy_from_slice(&[0, 0, 0, 0]);
        self.page_count = 0;
    }

    pub fn parse_recovery(bytes: Vec<u8>) -> Result<Option<RecoverMetadata>, SqliteError> {
        let mut cursor = SqliteCursor::new(&bytes);
        let magic = cursor.read_to(size_of!(u64) as _)?;
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

    pub fn persist_tail<J: SqliteFile>(&mut self, file: &J, start: usize) -> SqliteResult<()> {
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
pub struct RecoverMetadata {
    pub iterator: JournalIter,
    pub db_size: usize,
}
pub struct JournalIter {
    // TODO: Replace the allocation with immutable borrow
    bytes: Vec<u8>,
    start: usize,
    end: usize,
    hint: usize,
    count: usize,
    step_by: usize,
}
pub struct JournalPage<'a> {
    pub page_no: PageNo,
    pub data: &'a [u8],
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
        Self {
            hint,
            bytes: bytes.to_vec(),
            start: JOURNAL_HEADER_SIZE,
            end: JOURNAL_HEADER_SIZE + step_by,
            step_by,
            count: 0,
        }
    }

    pub fn owned(bytes: Vec<u8>, hint: usize, step_by: usize) -> Self {
        Self {
            hint,
            bytes,
            start: JOURNAL_HEADER_SIZE,
            end: JOURNAL_HEADER_SIZE + step_by,
            step_by,
            count: 0,
        }
    }

    pub fn iter<'a>(&'a mut self) -> Result<Option<JournalPage<'a>>, SqliteError> {
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

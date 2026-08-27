use crate::db::header::HEADER_STRING_SIZE;
use crate::errors::SqliteError;
use crate::vfs::disk::{DiskFile, DiskVfs};
use crate::vfs::file::SqliteFile;
use crate::vfs::{SqliteOptions, Vfs};
use crate::{SqliteCursor, size_of};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;

use super::pager::PageNo;

const JOURNAL_CAP: usize = 8;
// 1: 0..8
const JOURNAL_MAGIC: u64 = 0x4A4F55524E414C31; // DO NOT (HEX -> TEXT) IT
const JOURNAL_MAGIC_OFFSET: usize = 0;
// 2: 8..12
const PAGE_COUNT_OFFSET: usize = 8;
// 3: 12..16
const DATABASE_SIZE_OFFSET: usize = 12;

// 4: 16..20
const PAGE_SIZE_OFFSET: usize = 16;

const JOURNAL_HEADER_SIZE: usize = 20;

const PAGE_NUMBER_SIZE: usize = 4;
pub struct RawJournal {
    buffer: Vec<u8>,
    path: PathBuf,
    page_size: u16,
    db_name: String,
    page_count: u32,
    jfile: Option<DiskFile>,
}

pub struct JournalMeta {
    pub db_name: String,
    pub db_size: u32,
    pub p_size: u16,
    pub path: PathBuf,
}

impl RawJournal {
    pub fn new(
        JournalMeta {
            db_name,
            db_size,
            p_size,
            path,
        }: JournalMeta,
    ) -> Self {
        let mut buffer: Vec<u8> = Vec::with_capacity(
            JOURNAL_HEADER_SIZE + ((PAGE_NUMBER_SIZE + p_size as usize) * JOURNAL_CAP),
        );
        buffer.extend_from_slice(&u64::to_be_bytes(JOURNAL_MAGIC));
        buffer.extend_from_slice(&u32::to_be_bytes(0));
        buffer.extend_from_slice(&u32::to_be_bytes(db_size));
        buffer.extend_from_slice(&u32::to_be_bytes(p_size as _));
        Self {
            buffer,
            path,
            db_name,
            page_count: 0,
            page_size: p_size,
            jfile: None,
        }
    }

    pub fn init(&mut self) -> Result<(), SqliteError> {
        let file_path = self.path.join(format!("{}-journal", self.db_name));
        let mut file = Vfs::open(&mut DiskVfs, file_path, SqliteOptions::all())?;
        file.set_len(self.buffer.len())?;
        file.write_all_at(0, &self.buffer[0..JOURNAL_HEADER_SIZE])?;
        self.jfile = Some(file);
        Ok(())
    }
    pub fn is_init(&self) -> bool {
        self.jfile.is_some()
    }

    pub fn add_page(&mut self, page_no: PageNo, data: &[u8]) {
        assert!(self.jfile.is_some());
        self.buffer.extend_from_slice(&u32::to_be_bytes(page_no));
        self.buffer.extend_from_slice(data);
        self.page_count += 1;
    }
    pub fn commit(&mut self) -> Result<(), SqliteError> {
        assert!(self.jfile.is_some());
        let file = self.jfile.as_mut().unwrap();
        file.set_len(self.buffer.len())?;
        file.write_all_at(0 as _, &self.buffer)?;
        file.sync()?;
        let page_count_slice = &mut self.buffer[8..12];
        page_count_slice.copy_from_slice(&u32::to_ne_bytes(self.page_count));
        file.write_all_at(8, page_count_slice)?;
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

    pub fn rollback(&mut self) {
        unsafe {
            self.buffer.set_len(JOURNAL_HEADER_SIZE);
            self.buffer[8..12].copy_from_slice(&[0, 0, 0, 0]);
        }
    }

    pub fn recover(db_name: &str, path: PathBuf) -> Result<Option<RecoverMetadata>, SqliteError> {
        let file_path = path.join(format!("{}-journal", db_name));
        if !file_path.exists() {
            return Ok(None);
        }

        let mut file = Vfs::open(&mut DiskVfs, &file_path, SqliteOptions::default())?;
        let len = file.len()?;
        let mut bytes = vec![0u8; len as _];
        file.read_exact_at(0, &mut bytes)?;
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
            file_path,
        };
        Ok(Some(metadata))
    }

    pub fn destroy_internal(&mut self) -> Result<(), SqliteError> {
        let file_path = self.path.join(format!("{}-journal", self.db_name));
        std::fs::remove_file(file_path)?;
        self.jfile.take();
        Ok(())
    }
    pub fn destroy_external(path: PathBuf) -> Result<(), SqliteError> {
        std::fs::remove_file(path)?;
        Ok(())
    }
}

pub struct RecoverMetadata {
    pub iterator: JournalIter,
    pub db_size: usize,
    pub file_path: PathBuf,
}
pub struct JournalIter {
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
            .field("path", &self.path)
            .field("page_size", &self.page_size)
            .field("db_name", &self.db_name)
            .field("page_count", &self.page_count)
            .field("jfile", &self.jfile)
            .finish()
    }
}

struct J {
    inner: Option<RawJournal>,
}

impl Deref for J {
    type Target = RawJournal;
    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().unwrap()
    }
}
impl DerefMut for J {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().unwrap()
    }
}

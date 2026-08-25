use crate::SqliteCursor;
use crate::db::header::HEADER_STRING_SIZE;
use crate::errors::SqliteError;
use crate::vfs::disk::{DiskFile, DiskVfs};
use crate::vfs::file::SqliteFile;
use crate::vfs::{SqliteOptions, Vfs};
use std::path::PathBuf;

use super::pager::PageNo;

const JOURNAL_CAP: usize = 8;
// 1: 0..8
const JOURNAL_MAGIC: u64 = 0x4655434B494E474A; // DO NOT (HEX -> TEXT) IT
// 2: 8..12
const DEFAULT_JOURNAL_PAGE_COUNT: u32 = 0;
// 3: 12..16
const DATABASE_SIZE: u32 = 4;
const JOURNAL_HEADER_SIZE: usize = 16;

pub struct Journal {
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

impl Journal {
    pub fn new(
        JournalMeta {
            db_name,
            db_size,
            p_size,
            path,
        }: JournalMeta,
    ) -> Self {
        let mut buffer: Vec<u8> =
            Vec::with_capacity(JOURNAL_HEADER_SIZE + ((4 + p_size as usize) * JOURNAL_CAP));
        buffer.extend_from_slice(&u64::to_be_bytes(JOURNAL_MAGIC));
        buffer.extend_from_slice(&u32::to_be_bytes(DEFAULT_JOURNAL_PAGE_COUNT));
        buffer.extend_from_slice(&u32::to_be_bytes(db_size));
        // TODO, CHECK FOR RECOVER
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
        let file_path = self.path.join("simple2.db-journal");
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
        file.write_all_at(
            JOURNAL_HEADER_SIZE as _,
            &self.buffer[JOURNAL_HEADER_SIZE..],
        )?;
        file.sync()?;
        let page_count_slice = &mut self.buffer[8..12];
        page_count_slice.copy_from_slice(&u32::to_ne_bytes(self.page_count));
        file.write_all_at(8, page_count_slice)?;
        file.sync()?;
        Ok(())
    }

    pub fn make_iterator(&self) -> Result<JournalIter, SqliteError> {
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

    pub fn destroy(&mut self) {
        let file_path = self.path.join(format!("{}-journal", self.db_name));
        std::fs::remove_file(file_path);
        self.jfile.take();
    }
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
    pub fn new(bytes: &[u8], hint: usize, step_by: usize) -> Result<Self, SqliteError> {
        Ok(Self {
            hint,
            bytes: bytes.to_vec(),
            start: JOURNAL_HEADER_SIZE,
            end: JOURNAL_HEADER_SIZE + step_by,
            step_by,
            count: 0,
        })
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

use crate::errors::SqliteError;
use crate::vfs::disk::{DiskFile, DiskVfs};
use crate::vfs::file::SqliteFile;
use crate::vfs::{SqliteOptions, Vfs};
use std::path::PathBuf;

use super::pager::PageNo;

const JOURNAL_CAP: usize = 8;

const JOURNAL_MAGIC: u64 = 0x4655434B494E474A; // DO NOT (HEX -> TEXT) IT
const DEFAULT_JOURNAL_PAGE_COUNT: u32 = 0;
const DATABASE_SIZE: u32 = 4;
const JOURNAL_HEADER_SIZE: usize = 16;

struct Journal<'s> {
    buffer: Vec<u8>,
    path: PathBuf,
    page_size: u16,
    db_name: &'s str,
    page_count: u32,
    jfile: Option<DiskFile>,
}

struct JournalMeta<'s> {
    db_name: &'s str,
    db_size: u32,
    p_size: u16,
    path: PathBuf,
}

impl<'s> Journal<'s> {
    pub fn new(
        JournalMeta {
            db_name,
            db_size,
            p_size,
            path,
        }: JournalMeta<'s>,
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
        let file_path = self.path.join(format!("{}-journal", self.db_name));
        let mut file = Vfs::open(&mut DiskVfs, file_path, SqliteOptions::all())?;
        file.set_len(self.buffer.len())?;
        file.write_all_at(0, &self.buffer[0..JOURNAL_HEADER_SIZE])?;
        self.jfile = Some(file);
        Ok(())
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
}

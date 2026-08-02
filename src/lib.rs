#![allow(unused, dead_code)] // temp for now
mod bytes;
pub mod errors;
pub mod format;
mod macros;
mod pager;
mod util;
pub mod vfs;
use errors::SqliteDatabaseError;
use format::header::SqliteDatabaseHeader;
use format::overflow::compute_local_payload_size;
use format::page::BTreePage;
pub use format::varint::{decode_varint, encode_varint};
use std::path::Path;
use vfs::SqliteOptions;
pub type DbError = SqliteDatabaseError;

use self::format::page::PageNo;
use self::vfs::cursor::FileCursor;
use self::vfs::disk::{DiskFile, DiskVfs};
use self::vfs::file::SqliteFile;
use self::vfs::Vfs;

pub struct SqliteDatabase<S: SqliteFile> {
    source: S,
    header: SqliteDatabaseHeader,
}

impl SqliteDatabase<DiskFile> {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, SqliteDatabaseError> {
        let mut sqlite_default_vfs = DiskVfs;
        Self::with_source(sqlite_default_vfs, db_path)
    }
}

// 'f file source
impl<'f, S: SqliteFile> SqliteDatabase<S> {
    pub fn with_source<P: AsRef<Path>, V>(mut vfs: V, path: P) -> Result<Self, SqliteDatabaseError>
    where
        V: Vfs<File = S>,
    {
        let source = vfs.open(path, SqliteOptions::default())?;
        let header = SqliteDatabaseHeader::parse(&source)?;
        Ok(Self { source, header })
    }
    fn source(&'f self) -> &'f S {
        &self.source
    }
    fn cursor(&self) -> FileCursor<'_, S> {
        FileCursor::new(self.source())
    }
    fn cursor_at_offset(&self, offset: u64) -> FileCursor<'_, S> {
        FileCursor::with_offset(self.source(), offset)
    }
    pub fn header(&self) -> &'_ SqliteDatabaseHeader {
        &self.header
    }

    pub fn page(&mut self, page_no: PageNo) -> Result<BTreePage, SqliteDatabaseError> {
        self.validate_page(page_no, None::<fn(_) -> bool>)?;
        let page_size = self.header.database_page_size;
        let offset = page_size * (page_no - 1);

        let mut buff = vec![0u8; page_size as usize];
        self.source.read_exact_at(offset as u64, &mut buff)?;

        BTreePage::parse(
            buff,
            page_no,
            page_size as usize,
            (page_size - self.header.reserved_space as u32) as usize,
        )
    }
    pub fn usable_size(&self) -> u32 {
        self.header.database_page_size - self.header.reserved_space as u32
    }

    pub fn read_raw_page_into<B: AsMut<[u8]> + ?Sized>(
        &mut self,
        page_no: PageNo,
        buff: &mut B,
    ) -> Result<(), SqliteDatabaseError> {
        if page_no == 1 {
            return Err(SqliteDatabaseError::Corrupt(
                "Page no '1' cant be used as raw page".into(),
            ));
        }
        self.validate_page(page_no, None::<fn(_) -> bool>)?;
        let page_size = self.header.database_page_size;
        let offset = page_size * (page_no - 1);
        let buff = buff.as_mut();
        // self.file.seek(SeekFrom::Start(offset as u64))?;

        self.source.read_exact_at(offset as _, buff)?;

        Ok(())
    }

    fn validate_page<F>(
        &mut self,
        page_no: PageNo,
        exception: Option<F>,
    ) -> Result<(), SqliteDatabaseError>
    where
        F: Fn(PageNo) -> bool,
    {
        if let Some(exc) = exception {
            if exc(page_no) {
                return Err(SqliteDatabaseError::Corrupt("Exception Failed".into()));
            }
        }
        if page_no == 0 {
            return Err(SqliteDatabaseError::Corrupt(
                "page number cannot be zero".into(),
            ));
        } else if page_no > self.header.database_size_in_pages {
            return Err(SqliteDatabaseError::Corrupt(
                "page number is outside the database".into(),
            ));
        }

        Ok(())
    }

    pub fn page_count(&self) -> u32 {
        self.header.database_size_in_pages
    }
    pub fn page_size(&self) -> u32 {
        self.header.database_page_size
    }
}

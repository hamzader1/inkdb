#![allow(unused, dead_code)] // temp for now
// #![warn(unused_results)]
pub mod backend;
use backend::planner::plan::*;
mod bytes;
pub mod db;
use crate::db::header::SqliteDatabaseHeader;
pub mod record;
pub mod shell;
pub mod sql;
use sql::parse_ddl;
mod schema;
pub use schema::SqliteMaster;
pub mod errors;

mod macros;
pub mod pager;
pub mod storage;
mod util;

pub mod varint;
pub mod vfs;
use errors::SqliteError;

use std::path::Path;
pub use storage::sqlite_cursor::SqliteCursor;
use vfs::SqliteOptions;
pub type DbError = SqliteError;

use self::pager::pager::PageNo;
use self::vfs::Vfs;
use self::vfs::cursor::FileCursor;
use self::vfs::disk::{DiskFile, DiskVfs};
use self::vfs::file::SqliteFile;
use crate::pager::pager::Pager;

pub struct SqliteDatabase<F: SqliteFile> {
    pub pager: Pager<F>,
    header: SqliteDatabaseHeader,
}

impl SqliteDatabase<DiskFile> {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, SqliteError> {
        let sqlite_default_vfs = DiskVfs;
        Self::with_source(sqlite_default_vfs, db_path)
    }
    pub fn with_cache<P: AsRef<Path>>(db_path: P, cache_size: usize) -> Result<Self, SqliteError> {
        let sqlite_default_vfs = DiskVfs;
        Self::with_source_cache(sqlite_default_vfs, db_path, cache_size)
    }
}

// 'f file source
impl<F: SqliteFile> SqliteDatabase<F> {
    pub fn with_source<P: AsRef<Path>, V>(mut vfs: V, path: P) -> Result<Self, SqliteError>
    where
        V: Vfs<File = F>,
    {
        let source = vfs.open(path, SqliteOptions::default())?;
        let header = SqliteDatabaseHeader::parse(&source)?;
        let pager = Pager::new(
            source,
            header.database_page_size as _,
            (header.database_page_size - header.reserved_space as u32) as _,
            header.database_size_in_pages as _,
        )?;
        Ok(Self { pager, header })
    }
    pub fn with_source_cache<P: AsRef<Path>, V>(
        mut vfs: V,
        path: P,
        cache_size: usize,
    ) -> Result<Self, SqliteError>
    where
        V: Vfs<File = F>,
    {
        let source = vfs.open(path, SqliteOptions::default())?;
        let header = SqliteDatabaseHeader::parse(&source)?;
        let pager = Pager::with_cache(
            source,
            header.database_page_size as _,
            (header.database_page_size - header.reserved_space as u32) as _,
            header.database_size_in_pages as _,
            cache_size,
        )?;
        Ok(Self { pager, header })
    }
    fn source(&self) -> &F {
        &self.pager.source
    }
    fn cursor(&self) -> FileCursor<'_, F> {
        FileCursor::new(&self.pager.source)
    }
    fn cursor_at_offset(&self, offset: u64) -> FileCursor<'_, F> {
        FileCursor::with_offset(&self.pager.source, offset)
    }
    pub fn header(&self) -> &'_ SqliteDatabaseHeader {
        &self.header
    }

    // pub fn page(&mut self, page_no: PageNo) -> Result<BTreePage, SqliteError> {
    //     self.validate_page(page_no, None::<fn(_) -> bool>)?;
    //     let page_size = self.header.database_page_size;
    //     let offset = page_size * (page_no - 1);

    //     let mut buff = vec![0u8; page_size as usize];
    //     self.pager.source.read_exact_at(offset as u64, &mut buff)?;

    //     BTreePage::parse(
    //         buff,
    //         page_no,
    //         page_size as usize,
    //         (page_size - self.header.reserved_space as u32) as usize,
    //     )
    // }
    pub fn usable_size(&self) -> u32 {
        self.header.database_page_size - self.header.reserved_space as u32
    }

    pub fn read_raw_page_into<B: AsMut<[u8]> + ?Sized>(
        &mut self,
        page_no: PageNo,
        buff: &mut B,
    ) -> Result<(), SqliteError> {
        if page_no == 1 {
            return Err(SqliteError::Corrupt(
                "Page no '1' cant be used as raw page".into(),
            ));
        }
        self.validate_page(page_no, None::<fn(_) -> bool>)?;
        let page_size = self.header.database_page_size;
        let offset = page_size * (page_no - 1);
        let buff = buff.as_mut();
        // self.file.seek(SeekFrom::Start(offset as u64))?;

        self.pager.source.read_exact_at(offset as _, buff)?;

        Ok(())
    }

    fn validate_page<E>(&mut self, page_no: PageNo, exception: Option<E>) -> Result<(), SqliteError>
    where
        E: Fn(PageNo) -> bool,
    {
        if let Some(exc) = exception
            && exc(page_no)
        {
            return Err(SqliteError::Corrupt("Exception Failed".into()));
        }
        if page_no == 0 {
            return Err(SqliteError::Corrupt("page number cannot be zero".into()));
        } else if page_no > self.header.database_size_in_pages {
            return Err(SqliteError::Corrupt(
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

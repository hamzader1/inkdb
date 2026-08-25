use crate::SqliteMaster;
use crate::backend::analyze::Analyze;
use crate::backend::executor::RowWrapper;
use crate::backend::planner::plan::Plan;
// use crate::pager::pager::Pager;
// use crate::vfs::disk::DiskVfs;
use crate::errors::SqliteError;

pub mod header;
use crate::sql::lexer::Lexer;
use crate::sql::parser::Parser;
pub use crate::storage::sqlite_cursor::SqliteCursor;
use crate::vfs::mem::MemVfs;
use crate::vfs::{SqliteOptions, Vfs};
use header::SqliteDatabaseHeader;
use std::path::Path;
use std::rc::Rc;
pub type DbError = SqliteError;

use crate::pager::pager::Pager;
use crate::vfs::disk::{DiskFile, DiskVfs};

pub struct Database<F: crate::vfs::file::SqliteFile> {
    pub pager: Pager<F>,
    header: SqliteDatabaseHeader,
}

impl Database<DiskFile> {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, SqliteError> {
        let sqlite_default_vfs = DiskVfs;
        Self::with_source(sqlite_default_vfs, db_path)
    }
    pub fn with_cache<P: AsRef<Path>>(db_path: P, cache_size: usize) -> Result<Self, SqliteError> {
        let sqlite_default_vfs = DiskVfs;
        Self::with_source_cache(sqlite_default_vfs, db_path, cache_size)
    }
    pub fn execute(&mut self, query: &str) -> Result<(), SqliteError> {
        let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
        let query: Rc<str> = Rc::from(query.as_str());

        let lexer = Lexer::tokenize(&query)?;

        let res = Parser::parse(Rc::clone(&query), lexer)?;

        let sqlite_master = SqliteMaster::new(&mut self.pager)?;
        let resolved_query = Analyze::analyze(res, &sqlite_master)?;

        let mut plan = Plan::create_plan(resolved_query, &mut self.pager)?;
        while let Some(row) = plan.next(&mut self.pager)? {
            println!("{}", RowWrapper(row));
        }
        Ok(())
    }
}

impl<F: crate::vfs::file::SqliteFile> Database<F> {
    pub fn with_source<P: AsRef<Path>, V>(mut vfs: V, path: P) -> Result<Self, SqliteError>
    where
        V: Vfs<File = F>,
    {
        let source = vfs.open(path, SqliteOptions::default())?;
        let header = SqliteDatabaseHeader::parse(&source)?;
        let pager = Pager::new(
            source,
            header.database_page_size as usize,
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
}

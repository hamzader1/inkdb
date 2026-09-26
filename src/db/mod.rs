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
use crate::vfs::{SqliteOptions, Vfs};
use header::SqliteDatabaseHeader;
use std::path::Path;
use std::rc::Rc;
pub type DbError = SqliteError;

use crate::pager::pager::{HeaderCache, Pager};
use crate::vfs::disk::{DiskFile, DiskVfs};

pub struct Database<V: crate::vfs::Vfs> {
    pub pager: Pager<V>,
    header: SqliteDatabaseHeader,
}

impl Database<DiskVfs> {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, SqliteError> {
        let sqlite_default_vfs = DiskVfs;
        Self::with_source(sqlite_default_vfs, db_path)
    }
    pub fn with_cache<P: AsRef<Path>>(db_path: P, cache_size: usize) -> Result<Self, SqliteError> {
        let sqlite_default_vfs = DiskVfs;
        Self::with_source_cache(sqlite_default_vfs, db_path, cache_size)
    }
    pub fn execute(&mut self, query: &str) -> Result<(), SqliteError> {
        let query: Rc<str> = Rc::from(query);

        let lexer = Lexer::tokenize(&query)?;

        let res = Parser::parse(Rc::clone(&query), lexer)?;

        let sqlite_master = SqliteMaster::new(&mut self.pager)?;
        let resolved_query = Analyze::analyze(res, &sqlite_master)?;

        let mut plan = Plan::create_plan(resolved_query, &mut self.pager, &sqlite_master)?;
        while let Some(row) = plan.next(&mut self.pager, &sqlite_master)? {
            println!("{}", RowWrapper(row));
        }
        Ok(())
    }
}

impl<V: crate::vfs::Vfs> Database<V> {
    pub fn with_source<P: AsRef<Path>>(mut vfs: V, path: P) -> Result<Self, SqliteError> {
        let source = vfs.open(path, SqliteOptions::default())?;
        let header = SqliteDatabaseHeader::parse(&source)?;
        let pager = Pager::new(vfs, source, HeaderCache::from(header))?;
        Ok(Self { pager, header })
    }
    pub fn with_source_cache<P: AsRef<Path>>(
        mut vfs: V,
        path: P,
        cache_size: usize,
    ) -> Result<Self, SqliteError> {
        let source = vfs.open(path, SqliteOptions::default())?;
        let header = SqliteDatabaseHeader::parse(&source)?;
        let pager = Pager::with_cache(vfs, source, HeaderCache::from(header), cache_size)?;
        Ok(Self { pager, header })
    }
}

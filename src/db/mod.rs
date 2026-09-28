use crate::backend::analyze::Analyze;
use crate::backend::executor::{Row, RowWrapper};
use crate::backend::planner::plan::Plan;
use crate::backend::planner::prepared_plan::PreparedPlan;
use crate::{Master, InkResult};
// use crate::pager::pager::Pager;
// use crate::vfs::disk::DiskVfs;
use crate::errors::InkError;

pub mod header;
use crate::sql::lexer::Lexer;
use crate::sql::parser::Parser;
pub use crate::storage::cursor::MemCursor;
use crate::vfs::{InkOptions, Vfs};
use header::InkDatabaseHeader;
use std::path::Path;
use std::rc::Rc;
pub type DbError = InkError;

use crate::pager::pager::{HeaderCache, Pager};
use crate::vfs::disk::{DiskFile, DiskVfs};

pub struct Database<V: crate::vfs::Vfs> {
    pub pager: Pager<V>,
    pub master: Master,
    header: InkDatabaseHeader,
}

impl Database<DiskVfs> {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, InkError> {
        let default_vfs = DiskVfs;
        Self::with_source(default_vfs, db_path)
    }
    pub fn with_cache<P: AsRef<Path>>(db_path: P, cache_size: usize) -> Result<Self, InkError> {
        let default_vfs = DiskVfs;
        Self::with_source_cache(default_vfs, db_path, cache_size)
    }
}

impl<V: crate::vfs::Vfs> Database<V> {
    pub fn with_source<P: AsRef<Path>>(mut vfs: V, path: P) -> Result<Self, InkError> {
        let source = vfs.open(path, InkOptions::default())?;
        let header = InkDatabaseHeader::parse(&source)?;
        let mut pager = Pager::new(vfs, source, HeaderCache::from(header))?;
        let mut master = Master::new(&mut pager)?;
        Ok(Self {
            pager,
            master,
            header,
        })
    }
    pub fn with_source_cache<P: AsRef<Path>>(
        mut vfs: V,
        path: P,
        cache_size: usize,
    ) -> Result<Self, InkError> {
        let source = vfs.open(path, InkOptions::default())?;
        let header = InkDatabaseHeader::parse(&source)?;
        let mut pager = Pager::new(vfs, source, HeaderCache::from(header))?;
        let mut master = Master::new(&mut pager)?;
        Ok(Self {
            pager,
            master,
            header,
        })
    }
    pub fn execute(&mut self, query: &str) -> Result<Statement<'_, V>, InkError> {
        let query: Rc<str> = Rc::from(query);
        let lexer = Lexer::tokenize(&query)?;
        let res = Parser::parse(Rc::clone(&query), lexer)?;
        if self.master.is_dirty {
            self.master.parse(&mut self.pager)?;
        }
        let resolved_query = Analyze::analyze(res, &self.master)?;
        let plan = Plan::create_plan(resolved_query, &mut self.pager, &self.master)?;
        Ok(Statement {
            pager: &mut self.pager,
            master: &mut self.master,
            stmt: plan,
        })
    }
}

#[derive(Debug)]
pub struct Statement<'a, V: Vfs> {
    pager: &'a mut Pager<V>,
    master: &'a mut Master,
    stmt: PreparedPlan<V>,
}
impl<'a, V: Vfs> Statement<'a, V> {
    pub fn rows(&'a mut self) -> impl Iterator<Item = Result<Row, InkError>> + 'a {
        std::iter::from_fn(move || match self.stmt.next(self.pager, self.master) {
            Ok(Some(row)) => Some(Ok(row)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        })
    }
}

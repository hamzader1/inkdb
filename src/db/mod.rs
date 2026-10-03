use crate::backend::analyzer::Analyze;
use crate::backend::executor::{Row, RowWrapper};
use crate::backend::planner::plan::Plan;
use crate::backend::planner::prepared_plan::PreparedPlan;
use crate::errors::InkError;
use crate::{InkResult, Master};

pub mod header;
use crate::sql::lexer::Lexer;
use crate::sql::parser::Parser;
pub use crate::storage::cursor::MemCursor;
use crate::vfs::{InkOptions, Vfs};
use header::{DatabaseHeader, DbFormat};
use std::path::Path;
use std::rc::Rc;
pub type DbError = InkError;

use crate::pager::pager::{HeaderCache, Pager};
use crate::storage::page::{BTreePage, BTreePageType};
use crate::vfs::disk::DiskVfs;
use crate::vfs::file::InkFile;

pub struct Database<V: crate::vfs::Vfs> {
    pub pager: Pager<V>,
    pub master: Master,
    pub header: DatabaseHeader,
}

impl Database<DiskVfs> {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, InkError> {
        Self::open(DiskVfs, db_path)
    }
    pub fn with_cache<P: AsRef<Path>>(db_path: P, cache_size: usize) -> Result<Self, InkError> {
        let source = DiskVfs.open(&db_path, InkOptions::default())?;
        let header = DatabaseHeader::parse(&source)?;
        let pager = Pager::with_cache(DiskVfs, source, HeaderCache::from(header), cache_size)?;
        Self::load(pager, header)
    }
    pub fn open_or_create<P: AsRef<Path>>(db_path: P) -> Result<Self, InkError> {
        let fresh = match std::fs::metadata(db_path.as_ref()) {
            Err(_) => true,
            Ok(meta) => meta.len() == 0,
        };
        if !fresh {
            return Self::open(DiskVfs, db_path);
        }
        let format = DbFormat::for_path(&db_path);
        Self::create_with_vfs(DiskVfs, db_path, format)
    }
    pub fn create<P: AsRef<Path>>(db_path: P, format: DbFormat) -> Result<Self, InkError> {
        Self::create_with_vfs(DiskVfs, db_path, format)
    }
}

impl<V: crate::vfs::Vfs> Database<V> {
    pub fn open<P: AsRef<Path>>(mut vfs: V, path: P) -> Result<Self, InkError> {
        let source = vfs.open(path, InkOptions::default())?;
        let header = DatabaseHeader::parse(&source)?;
        let pager = Pager::new(vfs, source, HeaderCache::from(header))?;
        Self::load(pager, header)
    }

    pub fn with_source<P: AsRef<Path>>(vfs: V, path: P) -> Result<Self, InkError> {
        Self::open(vfs, path)
    }

    pub fn with_source_cache<P: AsRef<Path>>(
        vfs: V,
        path: P,
        cache_size: usize,
    ) -> Result<Self, InkError> {
        let mut vfs = vfs;
        let source = vfs.open(path, InkOptions::default())?;
        let header = DatabaseHeader::parse(&source)?;
        let pager = Pager::with_cache(vfs, source, HeaderCache::from(header), cache_size)?;
        Self::load(pager, header)
    }

    pub fn create_with_vfs<P: AsRef<Path>>(
        mut vfs: V,
        path: P,
        format: DbFormat,
    ) -> Result<Self, InkError> {
        let header = DatabaseHeader::default_for(format);
        let page_size = header.page_size() as usize;
        let usable = header.usable_size() as usize;
        let header_len = header.header_len();
        let source = vfs.open(&path, InkOptions::all())?;
        source.set_len(page_size)?;
        let mut first_page = vec![0u8; page_size];
        let header_bytes = header.serialize();
        first_page[..header_len].copy_from_slice(&header_bytes);
        BTreePage::new_from_raw_bytes(
            1,
            BTreePageType::LeafTable,
            &mut first_page[..],
            page_size,
            usable,
            header_len,
        )?;
        source.write_all_at(0, &first_page)?;
        source.sync()?;
        let pager = Pager::new(vfs, source, HeaderCache::from(header))?;
        Self::load(pager, header)
    }

    fn load(mut pager: Pager<V>, header: DatabaseHeader) -> Result<Self, InkError> {
        let master = Master::new(&mut pager)?;
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
        let resolved_query = Analyze::new(&self.master).analyze(res)?;
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

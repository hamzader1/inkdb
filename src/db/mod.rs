use crate::Master;
use crate::backend::analyzer::Analyze;
use crate::backend::executor::Row;
use crate::backend::planner::plan::Plan;
use crate::backend::planner::prepared_plan::PreparedPlan;
use crate::errors::InkError;

pub mod header;
use crate::sql::lexer::Lexer;
use crate::sql::parser::Parser;
pub use crate::storage::mem_cursor::MemCursor;
use crate::vfs::{InkOptions, Vfs};
use header::{DatabaseHeader, DbFormat};
use std::path::Path;
use std::rc::Rc;
pub type DbError = InkError;

use crate::pager::pager::{HeaderCache, Pager};
use crate::storage::page::{BTreePage, BTreePageType};
use crate::vfs::disk::DiskVfs;
use crate::vfs::file::InkFile;

/// The database handle, and the only way into the engine.
///
/// One of these owns everything a session needs: the [`pager`](`Pager`) that moves pages
/// between the file and memory, the [`schema`](`Master`) read out of the master table, and the
/// [`header`](DatabaseHeader) of the file it was opened from. Call [`Database::execute`] with a
/// statement and it takes care of parsing, checking and planning.
pub struct Database<V: crate::vfs::Vfs> {
    pager: Pager<V>,
    master: Master,
    header: DatabaseHeader,
}

impl<V: crate::vfs::Vfs> Database<V> {
    /// The pager, for the code that needs to reach pages directly instead of
    /// going through a statement (used mostly in tests).
    pub fn pager(&mut self) -> &mut Pager<V> {
        &mut self.pager
    }

    /// The schema as it was read when the file was opened, or as it was last
    /// reread after a statement changed it.
    pub fn master(&self) -> &Master {
        &self.master
    }

    /// The header of the file this database was opened from.
    pub fn header(&self) -> &DatabaseHeader {
        &self.header
    }
}

impl Database<DiskVfs> {
    /// Open a database that already exists on disk.
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, InkError> {
        Self::open(DiskVfs, db_path)
    }
    /// Open a database that already exists, with a buffer pool of `cache_size`
    /// frames instead of the default one. Worth it when the working set is
    /// larger than the default pool holds.
    pub fn with_cache<P: AsRef<Path>>(db_path: P, cache_size: usize) -> Result<Self, InkError> {
        let source = DiskVfs.open(&db_path, InkOptions::default())?;
        let header = DatabaseHeader::parse(&source)?;
        let pager = Pager::with_cache(DiskVfs, source, HeaderCache::from(header), cache_size)?;
        Self::load(pager, header)
    }
    /// Open the file, or lay down a fresh database when there is nothing there
    /// yet. A path that does not exist, or one that exists with zero length,
    /// counts as nothing: a half written file is never adopted, because a file
    /// that never got its header is not something to build on.
    ///
    /// The format of a new file follows its extension, so a name ending in
    /// `.inkdb` gets an Ink header and anything else gets a SQLite one.
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
    /// Create a brand new database at this path, in the format you name.
    ///
    /// For the SQLite format this is the same byte layout SQLite itself writes
    /// when it creates a file, which is what lets other tools open it. The Ink
    /// format is a shorter header that carries only what this engine needs.
    pub fn create<P: AsRef<Path>>(db_path: P, format: DbFormat) -> Result<Self, InkError> {
        Self::create_with_vfs(DiskVfs, db_path, format)
    }
}

impl<V: crate::vfs::Vfs> Database<V> {
    /// Open an existing file through the VFS of your choice.
    ///
    /// The header is read and checked first, so a file that is not a database
    /// is rejected before anything is planned around it.
    pub fn open<P: AsRef<Path>>(mut vfs: V, path: P) -> Result<Self, InkError> {
        let source = vfs.open(path, InkOptions::default())?;
        let header = DatabaseHeader::parse(&source)?;
        let pager = Pager::new(vfs, source, HeaderCache::from(header))?;
        Self::load(pager, header)
    }

    /// Old name for [`Database::open`], kept because callers still use it.
    pub fn with_source<P: AsRef<Path>>(vfs: V, path: P) -> Result<Self, InkError> {
        Self::open(vfs, path)
    }

    /// Open an existing file through the VFS of your choice, with a buffer pool
    /// of `cache_size` frames.
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

    /// Build a database that did not exist yet: write page one with a header,
    /// then turn that same page into the empty master table, so the file is
    /// usable the moment this returns.
    pub(crate) fn create_with_vfs<P: AsRef<Path>>(
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

    /// Finish opening: read the master table so the schema is ready before the
    /// caller gets the handle.
    fn load(mut pager: Pager<V>, header: DatabaseHeader) -> Result<Self, InkError> {
        let master = Master::new(&mut pager)?;
        Ok(Self {
            pager,
            master,
            header,
        })
    }

    /// Run one statement and hand back something to iterate over.
    ///
    /// The whole front end runs here in order: tokenize, parse, check the
    /// statement against the schema, then plan it. Nothing is executed yet, so
    /// an error about the statement itself comes from this call, while errors
    /// about the data come from the rows.
    pub fn execute(&mut self, query: &str) -> Result<Statement<'_, V>, InkError> {
        let query: Rc<str> = Rc::from(query);
        let lexer = Lexer::tokenize(&query)?;
        let res = Parser::parse(Rc::clone(&query), lexer)?;
        if self.master.is_dirty() {
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

// TODO: Take Rc<RefCell<Database>> instead
/// It borrows the database it came from, so only one statement runs at a time.
#[derive(Debug)]
pub struct Statement<'a, V: Vfs> {
    pager: &'a mut Pager<V>,
    master: &'a mut Master,
    stmt: PreparedPlan<V>,
}
impl<'a, V: Vfs> Statement<'a, V> {
    /// Pull rows out of the plan until it is done.
    pub fn rows(&'a mut self) -> impl Iterator<Item = Result<Row, InkError>> + 'a {
        std::iter::from_fn(move || match self.stmt.next(self.pager, self.master) {
            Ok(Some(row)) => Some(Ok(row)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        })
    }
}

use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::vfs::file::SqliteFile;

pub mod eval;
pub mod filter;
pub mod project;
pub mod scan;

pub trait Executor {
    fn next(&mut self, pager: &mut Pager<impl SqliteFile>) -> Result<Option<Row>, SqliteError>;
}

type Row = Vec<Value<'static>>;

use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;

pub mod eval;
pub mod filter;
pub mod project;
pub mod scan;

pub trait Executor {
    fn next(&mut self, pager: &mut Pager) -> Result<Option<Row>, SqliteError>;
}

impl Executor for Box<dyn Executor> {
    fn next(&mut self, pager: &mut Pager) -> Result<Option<Row>, SqliteError> {
        (**self).next(pager)
    }
}

pub type Row = Vec<Value<'static>>;

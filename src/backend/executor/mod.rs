use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;

pub mod create;
pub mod eval;
pub mod filter;
pub mod insert;
pub mod limit;
pub mod project;
pub mod scan;
pub mod transaction;
pub type Row = Vec<Value<'static>>;

#[repr(transparent)]
pub struct RowWrapper(pub Row);
impl std::fmt::Display for RowWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let row = &self.0;
        for (i, val) in row.iter().enumerate() {
            write!(f, "{}", val)?;
            if i < row.len() - 1 {
                write!(f, ", ")?;
            }
        }
        Ok(())
    }
}

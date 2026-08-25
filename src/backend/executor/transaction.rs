use crate::backend::executor::Row;
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;

#[derive(Debug)]
pub struct BeginTransaction;

impl BeginTransaction {
    pub fn next<F: SqliteFile>(&self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        pager.start_transaction();
        Ok(None)
    }
}

#[derive(Debug)]
pub struct CommitTransaction;

impl CommitTransaction {
    pub fn next<F: SqliteFile>(&self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        if pager.in_transaction() {
            pager.commit()?;
        }
        Ok(None)
    }
}

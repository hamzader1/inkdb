use crate::SqliteResult;
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
        if pager.in_transaction() {
            return Err(SqliteError::TransactionAlreadyStarted);
        }
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
            Ok(None)
        } else {
            Err(SqliteError::NoActiveTransaction)
        }
    }
}

#[derive(Debug)]
pub struct RollBackTransaction;
impl RollBackTransaction {
    pub fn next<F: SqliteFile>(&self, pager: &mut Pager<F>) -> SqliteResult<Option<Row>> {
        if !pager.in_transaction() {
            return Err(SqliteError::NoActiveTransaction);
        }
        pager.rollback()?;
        Ok(None)
    }
}

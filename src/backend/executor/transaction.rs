use crate::SqliteResult;
use crate::backend::executor::Row;
use crate::errors::SqliteError;
use crate::vfs::Vfs;

use super::context::ExecCtx;

#[derive(Debug)]
pub struct BeginTransaction;

impl BeginTransaction {
    pub fn next<V: Vfs>(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, SqliteError> {
        if ctx.pager.in_transaction() {
            return Err(SqliteError::TransactionAlreadyStarted);
        }
        ctx.pager.start_transaction();
        Ok(None)
    }
}

#[derive(Debug)]
pub struct CommitTransaction;

impl CommitTransaction {
    pub fn next<V: Vfs>(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, SqliteError> {
        if ctx.pager.in_transaction() {
            ctx.pager.commit()?;
            Ok(None)
        } else {
            Err(SqliteError::NoActiveTransaction)
        }
    }
}

#[derive(Debug)]
pub struct RollBackTransaction;
impl RollBackTransaction {
    pub fn next<V: Vfs>(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        if !ctx.pager.in_transaction() {
            return Err(SqliteError::NoActiveTransaction);
        }
        ctx.pager.rollback()?;
        Ok(None)
    }
}

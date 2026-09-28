use crate::InkResult;
use crate::backend::executor::Row;
use crate::errors::InkError;
use crate::vfs::Vfs;

use super::context::ExecCtx;

#[derive(Debug)]
pub struct BeginTransaction;

impl BeginTransaction {
    pub fn next<V: Vfs>(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, InkError> {
        if ctx.pager.in_transaction() {
            return Err(InkError::TransactionAlreadyStarted);
        }
        ctx.pager.start_transaction();
        Ok(None)
    }
}

#[derive(Debug)]
pub struct CommitTransaction;

impl CommitTransaction {
    pub fn next<V: Vfs>(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, InkError> {
        if ctx.pager.in_transaction() {
            ctx.pager.commit()?;
            Ok(None)
        } else {
            Err(InkError::NoActiveTransaction)
        }
    }
}

#[derive(Debug)]
pub struct RollBackTransaction;
impl RollBackTransaction {
    pub fn next<V: Vfs>(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if !ctx.pager.in_transaction() {
            return Err(InkError::NoActiveTransaction);
        }
        ctx.pager.rollback()?;
        ctx.master.is_dirty = true;
        Ok(None)
    }
}

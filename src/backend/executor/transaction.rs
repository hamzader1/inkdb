use crate::InkResult;
use crate::backend::executor::Row;
use crate::errors::InkError;
use crate::vfs::Vfs;

use super::context::ExecCtx;

/// Starts a write transaction.
///
/// A transaction that is already open is an error rather than being joined, so
/// each BEGIN has to be matched by its own COMMIT or ROLLBACK.
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

/// Ends a write transaction, writing everything the transaction changed.
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

/// Undoes a write transaction, putting every page it touched back the way it
/// was.
#[derive(Debug)]
pub struct RollBackTransaction;
impl RollBackTransaction {
    pub fn next<V: Vfs>(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if !ctx.pager.in_transaction() {
            return Err(InkError::NoActiveTransaction);
        }
        ctx.pager.rollback()?;
        ctx.master.mark_dirty();
        Ok(None)
    }
}

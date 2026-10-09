use crate::InkResult;
use crate::backend::planner::plan::Plan;
use crate::pager::pager::PageNo;
use crate::storage::btree::BTree;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;

/// Removes each row its child yields.
///
/// The row is deleted by its key, which the child still carries even after a
/// projection, and the row is then handed on so anything above can finish with
/// it.
#[derive(Debug)]
pub struct Delete<V: Vfs> {
    child: Box<Plan<V>>,
    root_page: PageNo,
}

impl<V: Vfs> Delete<V> {
    pub fn new(child: Box<Plan<V>>, root_page: PageNo) -> Self {
        Self { child, root_page }
    }
    /// The operator this one pulls from.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    /// The root page of the table rows are deleted from.
    pub fn root_page(&self) -> PageNo {
        self.root_page
    }
    /// Delete one row from the tree and pass it on.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if let Some(row) = self.child.next(ctx)? {
            let mut btree = BTree::new(self.root_page, ctx.pager);
            btree.delete(row.key().into())?;
            return Ok(Some(row));
        }
        Ok(None)
    }
}

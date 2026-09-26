use crate::SqliteResult;
use crate::backend::planner::plan::Plan;
use crate::pager::pager::PageNo;
use crate::storage::btree::BTree;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;

#[derive(Debug)]
pub struct Delete<V: Vfs> {
    child: Box<Plan<V>>,
    root_page: PageNo,
}

impl<V: Vfs> Delete<V> {
    pub fn new(child: Box<Plan<V>>, root_page: PageNo) -> Self {
        Self { child, root_page }
    }
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    pub fn root_page(&self) -> PageNo {
        self.root_page
    }
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        while let Some(row) = self.child.next(ctx)? {
            let mut btree = BTree::new(self.root_page, ctx.pager);
            btree.delete(row.key.into())?;
        }
        Ok(None)
    }
}

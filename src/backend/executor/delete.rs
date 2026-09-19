use crate::SqliteResult;
use crate::backend::planner::plan::Plan;
use crate::pager::pager::{PageNo, Pager};
use crate::record::SqlType;
use crate::sql::parser::ExprArena;
use crate::storage::btree::BTree;
use crate::vfs::Vfs;

use super::Row;

#[derive(Debug)]
pub struct Delete<V: Vfs> {
    child: Box<Plan<V>>,
    root_page: PageNo,
}

impl<V: Vfs> Delete<V> {
    pub fn new(child: Box<Plan<V>>, root_page: PageNo) -> Self {
        Self { child, root_page }
    }
    /// Child subtree for optimizer traversal.
    pub fn child_mut(&mut self) -> &mut Plan<V> {
        &mut self.child
    }
    pub fn root_page(&self) -> PageNo {
        self.root_page
    }
    pub fn next(
        &mut self,
        pager: &mut Pager<V>,
        arena: Option<&ExprArena>,
    ) -> SqliteResult<Option<Row>> {
        while let Some(row) = self.child.next(pager, arena)? {
            let mut btree = BTree::new(self.root_page, pager);
            btree.delete(row.key.into_sqlite_value())?;
        }
        Ok(None)
    }
}

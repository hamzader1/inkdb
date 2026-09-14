use crate::SqliteResult;
use crate::backend::planner::plan::Plan;
use crate::pager::pager::{PageNo, Pager};
use crate::record::SqlType;
use crate::sql::parser::ExprArena;
use crate::storage::btree::{BTree, BTreeCursor};
use crate::vfs::file::SqliteFile;

use super::Row;

#[derive(Debug)]
pub struct Delete<F: SqliteFile> {
    child: Box<Plan<F>>,
    root_page: PageNo,
}

impl<F: SqliteFile> Delete<F> {
    pub fn new(child: Box<Plan<F>>, root_page: PageNo) -> Self {
        Self { child, root_page }
    }
    /// Child subtree for optimizer traversal.
    pub fn child_mut(&mut self) -> &mut Plan<F> {
        &mut self.child
    }
    pub fn next(
        &mut self,
        pager: &mut Pager<F>,
        arena: Option<&ExprArena>,
    ) -> SqliteResult<Option<Row>> {
        while let Some(row) = self.child.next(pager, arena)? {
            let mut btree = BTree::new(self.root_page, pager);
            btree.delete(row.key.into_sqlite_value())?;
        }
        Ok(None)
    }
}

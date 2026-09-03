use crate::SqliteResult;
use crate::backend::planner::plan::Plan;
use crate::pager::pager::{PageNo, Pager};
use crate::record::SqlType;
use crate::sql::parser::ExprArena;
use crate::storage::btree::BTree;
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
    pub fn next(
        &mut self,
        pager: &mut Pager<F>,
        arena: Option<&ExprArena>,
    ) -> SqliteResult<Option<Row>> {
        let mut ids = Vec::new();
        while let Some(row) = self.child.next(pager, arena)? {
            ids.push(row.key);
        }
        let mut btree = BTree::new(self.root_page, pager);
        for key in ids {
            btree.delete(key.into_sqlite_value())?;
        }
        Ok(None)
    }
}

use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

use super::Row;

#[derive(Debug)]
pub struct Limit<V: Vfs> {
    child: Box<Plan<V>>,
    pub limit: usize,
    is_done: bool,
}

impl<V: Vfs> Limit<V> {
    pub fn new(child: Box<Plan<V>>, limit: usize) -> Self {
        Self {
            child,
            limit,
            is_done: false,
        }
    }
    /// Child subtree for optimizer traversal.
    pub fn child_mut(&mut self) -> &mut Plan<V> {
        &mut self.child
    }
    pub fn next(
        &mut self,
        pager: &mut Pager<V>,
        arena: &ExprArena,
    ) -> Result<Option<Row>, SqliteError> {
        if self.is_done {
            return Ok(None);
        }
        if self.limit > 0
            && let Some(row) = self.child.next(pager, Some(arena))?
        {
            self.limit -= 1;
            if self.limit == 0 {
                self.is_done = true
            };
            return Ok(Some(row));
        }
        Ok(None)
    }
}

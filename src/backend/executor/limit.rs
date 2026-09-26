use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;

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
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, SqliteError> {
        if self.is_done {
            return Ok(None);
        }
        if self.limit > 0
            && let Some(row) = self.child.next(ctx)?
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

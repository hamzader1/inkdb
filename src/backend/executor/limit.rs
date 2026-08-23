use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;

use super::Row;

#[derive(Debug)]
pub struct Limit<F: SqliteFile> {
    child: Box<Plan<F>>,
    limit: usize,
    is_done: bool,
}

impl<F: SqliteFile> Limit<F> {
    pub fn new(child: Box<Plan<F>>, limit: usize) -> Self {
        Self {
            child,
            limit,
            is_done: false,
        }
    }
    pub fn next(
        &mut self,
        pager: &mut Pager<F>,
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

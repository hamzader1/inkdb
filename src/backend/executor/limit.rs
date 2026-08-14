use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;

use super::Row;

#[derive(Debug)]
pub struct Limit {
    child: Box<Plan>,
    limit: usize,
    is_done: bool,
}

impl Limit {
    pub fn new(child: Box<Plan>, limit: usize) -> Self {
        Self {
            child,
            limit,
            is_done: false,
        }
    }
    pub fn next(
        &mut self,
        pager: &mut Pager,
        arena: &ExprArena,
    ) -> Result<Option<Row>, SqliteError> {
        if self.is_done {
            return Ok(None);
        }
        if self.limit > 0
            && let Some(row) = self.child.next(pager, arena)?
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

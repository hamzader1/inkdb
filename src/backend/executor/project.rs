use crate::backend::planner::plan::Plan;
use crate::record::Value;
use crate::sql::parser::Arena;

use super::eval::Eval;

#[derive(Debug)]
pub struct Project {
    pub child: Box<Plan>,
    arena: Arena,
    columns: Vec<usize>,
}

impl Project {
    pub fn new(child: Box<Plan>, arena: Arena, columns: Vec<usize>) -> Self {
        Self {
            child,
            arena,
            columns,
        }
    }
}
impl Project {
    pub fn next(
        &mut self,
        pager: &mut crate::pager::pager::Pager,
    ) -> Result<Option<super::Row>, crate::errors::SqliteError> {
        if let Some(row) = self.child.next(pager)? {
            let output_row: Vec<Value<'static>> = self
                .columns
                .iter()
                .map(|i| Eval::eval(&self.arena, *i, &row))
                .collect();
            return Ok(Some(output_row));
        }

        Ok(None)
    }
}

use crate::backend::planner::plan::Plan;
use crate::record::Value;
use crate::sql::parser::ExprArena;

use super::eval::Eval;

#[derive(Debug)]
pub struct Project {
    pub child: Box<Plan>,
    columns: Vec<usize>,
}

impl Project {
    pub fn new(child: Box<Plan>, columns: Vec<usize>) -> Self {
        Self { child, columns }
    }
}
impl Project {
    pub fn next(
        &mut self,
        pager: &mut crate::pager::pager::Pager,
        arena: &ExprArena,
    ) -> Result<Option<super::Row>, crate::errors::SqliteError> {
        if let Some(row) = self.child.next(pager, Some(arena))? {
            let output_row: Vec<Value<'static>> = self
                .columns
                .iter()
                .map(|i| Eval::eval_row(arena, *i, &row))
                .collect();
            return Ok(Some(output_row));
        }

        Ok(None)
    }
}

use crate::backend::planner::plan::Plan;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;

use super::Row;
use super::eval::Eval;

#[derive(Debug)]
pub struct Project<F: SqliteFile> {
    pub child: Box<Plan<F>>,
    columns: Vec<usize>,
}

impl<F: SqliteFile> Project<F> {
    pub fn new(child: Box<Plan<F>>, columns: Vec<usize>) -> Self {
        Self { child, columns }
    }
}
impl<F: SqliteFile> Project<F> {
    pub fn next(
        &mut self,
        pager: &mut Pager<F>,
        arena: &ExprArena,
    ) -> Result<Option<Row>, crate::errors::SqliteError> {
        if let Some(mut row) = self.child.next(pager, Some(arena))? {
            let output_row: Vec<Value<'static>> = self
                .columns
                .iter()
                .map(|i| Eval::eval_row(arena, *i, &row))
                .collect();
            row.data = output_row;
            return Ok(Some(row));
        }

        Ok(None)
    }
}

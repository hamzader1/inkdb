use crate::backend::executor::eval::Eval;
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;

use super::Row;

#[derive(Debug)]
pub struct Filter<F: SqliteFile> {
    child: Box<Plan<F>>,
    predict: usize,
}
impl<F: SqliteFile> Filter<F> {
    pub fn new(child: Box<Plan<F>>, predict: usize) -> Self {
        Self { child, predict }
    }
}

impl<F: SqliteFile> Filter<F> {
    pub fn next(
        &mut self,
        pager: &mut Pager<F>,
        arena: &ExprArena,
    ) -> Result<Option<Row>, SqliteError> {
        loop {
            let row = match self.child.next(pager, Some(arena))? {
                Some(row) => row,
                _ => return Ok(None),
            };
            if Eval::eval_row(arena, self.predict, &row).to_bool() {
                return Ok(Some(row));
            }
        }
    }
}

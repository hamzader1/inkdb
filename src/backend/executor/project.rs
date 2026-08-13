use crate::record::Value;
use crate::sql::parser::Arena;

use super::Executor;
use super::eval::Eval;

pub struct Project<E: Executor> {
    child: E,
    arena: Arena,
    columns: Vec<usize>,
}
impl<E: Executor> Project<E> {
    pub fn new(child: E, arena: Arena, columns: Vec<usize>) -> Self {
        Self {
            child,
            arena,
            columns,
        }
    }
}
impl<E: Executor> Executor for Project<E> {
    fn next(
        &mut self,
        pager: &mut crate::pager::pager::Pager,
    ) -> Result<Option<super::Row>, crate::errors::SqliteError> {
        if let Some(row) = self.child.next(pager)? {
            let output_row: Vec<Value<'static>> = self
                .columns
                .iter()
                .map(|i| Eval::eval(&self.arena, *i, &row))
                .collect();
            // for
            return Ok(Some(output_row));
        }
        Ok(None)
    }
}

// take the row
// apply eval on it
// return val
// replace it with the same index

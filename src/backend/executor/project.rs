use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

use super::Row;
use super::eval::Eval;

#[derive(Debug)]
pub struct Project<V: Vfs> {
    pub child: Box<Plan<V>>,
    columns: Vec<usize>,
}

impl<V: Vfs> Project<V> {
    pub fn new(child: Box<Plan<V>>, columns: Vec<usize>) -> Self {
        Self { child, columns }
    }
    pub fn columns(&self) -> &[usize] {
        &self.columns
    }
}
impl<V: Vfs> Project<V> {
    pub fn next(
        &mut self,
        pager: &mut Pager<V>,
        arena: &ExprArena,
    ) -> Result<Option<Row>, crate::errors::SqliteError> {
        if let Some(mut row) = self.child.next(pager, Some(arena))? {
            let output_row: Vec<Value<'static>> = self
                .columns
                .iter()
                .map(|i| {
                    let value = Eval::eval(arena, *i, Some(&row))?;
                    Ok(value.into_static())
                })
                .collect::<Result<Vec<_>, SqliteError>>()?;
            row.data = output_row;
            return Ok(Some(row));
        }

        Ok(None)
    }
}

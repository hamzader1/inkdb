use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::record::Value;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;
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
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
}
impl<V: Vfs> Project<V> {
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, SqliteError> {
        if let Some(mut row) = self.child.next(ctx)? {
            let output_row: Vec<Value<'static>> = self
                .columns
                .iter()
                .map(|i| {
                    let value = Eval::eval(ctx.arena, *i, Some(&row))?;
                    Ok(value.into_static())
                })
                .collect::<Result<Vec<_>, SqliteError>>()?;
            row.data = output_row;
            return Ok(Some(row));
        }

        Ok(None)
    }
}

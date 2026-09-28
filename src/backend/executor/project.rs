use crate::backend::planner::plan::Plan;
use crate::errors::InkError;
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
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, InkError> {
        if let Some(row) = self.child.next(ctx)? {
            let key = row.key();
            let output_row: Vec<Value<'static>> = self
                .columns
                .iter()
                .map(|i| {
                    let value = Eval::eval(ctx.arena, *i, Some(&row))?;
                    Ok(value.into_static())
                })
                .collect::<Result<Vec<_>, InkError>>()?;
            return Ok(Some(Row::new(key, output_row)));
        }

        Ok(None)
    }
}

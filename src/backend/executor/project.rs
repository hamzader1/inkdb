use crate::backend::planner::plan::Plan;
use crate::errors::InkError;
use crate::record::Value;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;
use super::eval::Eval;

/// Turns the rows of its child into the rows the query asked for.
///
/// One expression per output column, each run over the input row. This is also
/// where a table row becomes the shape the rest of the plan expects.
#[derive(Debug)]
pub struct Project<V: Vfs> {
    pub(crate) child: Box<Plan<V>>,
    columns: Box<[usize]>,
}

impl<V: Vfs> Project<V> {
    pub fn new(child: Box<Plan<V>>, columns: Box<[usize]>) -> Self {
        Self { child, columns }
    }
    /// The output columns, one arena index each.
    pub fn columns(&self) -> &[usize] {
        &self.columns
    }
    /// The operator this one pulls from.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
}
impl<V: Vfs> Project<V> {
    /// Run the output expressions over one row from below.
    ///
    /// The key is carried through unchanged, so a projection keeps the row ids it
    /// was given and a later operator can still find the row it came from.
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

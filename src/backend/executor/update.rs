use super::super::planner::plan::Plan;
use crate::{
    InkResult,
    backend::executor::{Row, context::ExecCtx, eval::Eval},
    sql::ast::Expr,
    vfs::Vfs,
};

#[derive(Debug)]
pub struct Update<V: Vfs> {
    pub(crate) child: Box<Plan<V>>,
    pub(crate) affected_columns: Box<[(usize, usize)]>, /*(ColumnIndex, NewValue)*/
}
impl<V: Vfs> Update<V> {
    pub fn new(child: Box<Plan<V>>, affected_columns: Box<[(usize, usize)]>) -> Self {
        Self {
            child,
            affected_columns,
        }
    }
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if let Some(row) = self.child.next(ctx)? {
            let mut row_values = row.to_values()?;
            for (column_index, new_value_index) in self.affected_columns.iter() {
                let new_val = Eval::eval(ctx.arena, *new_value_index, Some(&row))?.into_static();
                /*Temporary*/
                let Expr::ColumnRef(index) = ctx.arena.nodes[*column_index] else {
                    unreachable!()
                };
                row_values[index] = new_val;
            }
            return Ok(Some(Row::new(0, row_values)));
        }
        Ok(None)
    }
}

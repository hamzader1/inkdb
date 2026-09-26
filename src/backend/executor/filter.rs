use crate::backend::executor::eval::Eval;
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;

#[derive(Debug)]
pub struct Filter<V: Vfs> {
    child: Box<Plan<V>>,
    predicate: usize,
}
impl<V: Vfs> Filter<V> {
    pub fn new(child: Box<Plan<V>>, predicate: usize) -> Self {
        Self { child, predicate }
    }
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    pub fn child_mut(&mut self) -> &mut Plan<V> {
        &mut self.child
    }
    pub fn predicate(&self) -> usize {
        self.predicate
    }
}

impl<V: Vfs> Filter<V> {
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, SqliteError> {
        loop {
            let row = match self.child.next(ctx)? {
                Some(row) => row,
                _ => return Ok(None),
            };
            if Eval::eval(ctx.arena, self.predicate, Some(&row))?.to_bool() {
                return Ok(Some(row));
            }
        }
    }
}

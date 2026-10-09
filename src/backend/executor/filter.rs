use crate::backend::executor::eval::Eval;
use crate::backend::planner::plan::Plan;
use crate::errors::InkError;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;

/// Passes on only the rows whose predicate holds.
///
/// The predicate is an arena index, and it is run over the row both here and,
/// when it was pushed down, inside the scan that feeds this. Checking it twice
/// costs a little, in exchange for a scan that can hand over only some of its
/// rows and a filter that is right whatever the scan did.
#[derive(Debug)]
pub struct Filter<V: Vfs> {
    child: Box<Plan<V>>,
    predicate: usize,
}
impl<V: Vfs> Filter<V> {
    pub fn new(child: Box<Plan<V>>, predicate: usize) -> Self {
        Self { child, predicate }
    }
    /// The operator this one pulls from.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    /// Take the child out, for a plan that wants to keep it and drop the filter.
    pub fn into_child(self) -> Box<Plan<V>> {
        self.child
    }
    /// The child, to be replaced by a scan that can answer the predicate.
    pub fn child_mut(&mut self) -> &mut Plan<V> {
        &mut self.child
    }
    /// The predicate rows are checked against, as an arena index.
    pub fn predicate(&self) -> usize {
        self.predicate
    }
}

impl<V: Vfs> Filter<V> {
    /// Pull rows from below until one passes the predicate, then hand it on.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, InkError> {
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

use crate::{
    InkResult,
    backend::{
        executor::{Row, context::ExecCtx},
        planner::plan::Plan,
    },
    vfs::Vfs,
};

/// Marks the schema stale after the plan below it has done its work.
///
/// Dropping a table rewrites the catalog, so the schema held in memory no longer
/// matches the file, and the next statement has to read it again. This sits at
/// the top of a drop plan, and it only runs its child through.
#[derive(Debug)]
pub struct DropTbl<V: Vfs> {
    child: Box<Plan<V>>,
    // root_page: u32,
}
impl<V: Vfs> DropTbl<V> {
    pub fn new(child: Box<Plan<V>>) -> Self {
        Self { child }
    }
    /// The operator this one pulls from.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    /// Run the child to the end, then mark the schema stale.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        while self.child.next(ctx)?.is_some() {}
        ctx.master.mark_dirty();
        Ok(None)
    }
}

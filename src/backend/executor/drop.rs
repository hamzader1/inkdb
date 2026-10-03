use crate::{
    InkResult,
    backend::{
        executor::{Row, context::ExecCtx},
        planner::plan::Plan,
    },
    vfs::Vfs,
};

#[derive(Debug)]
pub struct DropTbl<V: Vfs> {
    child: Box<Plan<V>>,
    // root_page: u32,
}
impl<V: Vfs> DropTbl<V> {
    pub fn new(child: Box<Plan<V>>) -> Self {
        Self { child }
    }
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        while self.child.next(ctx)?.is_some() {}
        ctx.master.is_dirty = true;
        Ok(None)
    }
}

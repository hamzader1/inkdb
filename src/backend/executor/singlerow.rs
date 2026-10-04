use crate::InkResult;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;

#[derive(Debug, Default)]
pub struct SingleRow {
    done: bool,
}

impl SingleRow {
    pub fn new() -> Self {
        Self { done: false }
    }

    pub fn next<V: Vfs>(&mut self, _ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if self.done {
            return Ok(None);
        }
        self.done = true;
        Ok(Some(Row::new(0, Vec::new())))
    }
}

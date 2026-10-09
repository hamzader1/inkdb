use crate::InkResult;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;

/// One empty row, which is what a SELECT with no FROM clause runs over.
#[derive(Debug, Default)]
pub struct SingleRow {
    done: bool,
}

impl SingleRow {
    pub fn new() -> Self {
        Self { done: false }
    }

    /// Yield the one row, and nothing after it.
    pub fn next<V: Vfs>(&mut self, _ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if self.done {
            return Ok(None);
        }
        self.done = true;
        Ok(Some(Row::new(0, Vec::new())))
    }
}

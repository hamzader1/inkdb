use crate::SqliteResult;
use crate::backend::planner::plan::Plan;
use crate::record::Value;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;
use super::eval::Eval;

#[derive(Debug)]
pub struct Count<V: Vfs> {
    child: Box<Plan<V>>,
    arg: Option<usize>,
    is_done: bool,
}

impl<V: Vfs> Count<V> {
    pub fn new(child: Box<Plan<V>>, arg: Option<usize>) -> Self {
        Self {
            child,
            arg,
            is_done: false,
        }
    }

    pub fn child(&self) -> &Plan<V> {
        &self.child
    }

    pub fn arg(&self) -> Option<usize> {
        self.arg
    }

    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        if self.is_done {
            return Ok(None);
        }
        self.is_done = true;
        let mut count: i64 = 0;
        while let Some(row) = self.child.next(ctx)? {
            let counted = match self.arg {
                None => true,
                Some(arg) => !matches!(Eval::eval(ctx.arena, arg, Some(&row))?, Value::Null),
            };
            if counted {
                count += 1;
            }
        }
        Ok(Some(Row::new(0, vec![Value::Integer(count)])))
    }
}

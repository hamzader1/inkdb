use crate::InkResult;
use crate::backend::planner::plan::Plan;
use crate::record::Value;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;
use super::eval::Eval;

/// Counts the rows of its child.
///
/// A count with an argument only counts the rows where that expression is not
/// NULL, which is not the same as counting every row. The whole child is walked
/// on the first call and one row comes out, since the answer is not known until
/// the last one.
/*Limited*/
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

    /// Walk the whole child, then yield the one count.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
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

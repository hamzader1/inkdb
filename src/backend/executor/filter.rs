use crate::backend::executor::eval::Eval;
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

use super::Row;

#[derive(Debug)]
pub struct Filter<V: Vfs> {
    child: Box<Plan<V>>,
    predicate: usize,
}
impl<V: Vfs> Filter<V> {
    pub fn new(child: Box<Plan<V>>, predicate: usize) -> Self {
        Self { child, predicate }
    }
    pub fn child_mut(&mut self) -> &mut Plan<V> {
        &mut self.child
    }
    pub fn predicate(&self) -> usize {
        self.predicate
    }
}

impl<V: Vfs> Filter<V> {
    pub fn next(
        &mut self,
        pager: &mut Pager<V>,
        arena: &ExprArena,
    ) -> Result<Option<Row>, SqliteError> {
        loop {
            let row = match self.child.next(pager, Some(arena))? {
                Some(row) => row,
                _ => return Ok(None),
            };
            if Eval::eval(arena, self.predicate, Some(&row))?.to_bool() {
                return Ok(Some(row));
            }
        }
    }
}

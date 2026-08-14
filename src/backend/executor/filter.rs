use crate::backend::executor::eval::Eval;
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::Arena;

use super::Row;

#[derive(Debug)]
pub struct Filter {
    child: Box<Plan>,
    arena: Arena,
    predict: usize,
}
impl Filter {
    pub fn new(child: Box<Plan>, arena: Arena, predict: usize) -> Self {
        Self {
            child,
            arena,
            predict,
        }
    }
}

impl Filter {
    pub fn next(&mut self, pager: &mut Pager) -> Result<Option<Row>, SqliteError> {
        loop {
            let row = match self.child.next(pager)? {
                Some(row) => row,
                _ => return Ok(None),
            };
            if Eval::eval(&self.arena, self.predict, &row).to_bool() {
                return Ok(Some(row));
            } else {
                // dbg!(self.predict);
                // dbg!(Eval::eval(&self.arena, self.predict, &row));
            }
        }
    }
}

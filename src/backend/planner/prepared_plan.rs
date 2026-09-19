use crate::backend::executor::Row;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

use super::plan::Plan;

#[derive(Debug)]
pub struct PreparedPlan<V: Vfs> {
    pub parent: Plan<V>,
    pub arena: Option<ExprArena>,
}
impl<V: Vfs> PreparedPlan<V> {
    pub fn new(parent: Plan<V>, arena: Option<ExprArena>) -> Self {
        Self { parent, arena }
    }
    pub fn next(&mut self, pager: &mut Pager<V>) -> Result<Option<Row>, SqliteError> {
        match pager.start_transaction() {
            true => {
                let parent_res = self.parent.next(pager, self.arena.as_ref());
                match parent_res {
                    Ok(_) => pager.commit()?,
                    _ => pager.rollback()?,
                }
                parent_res
            }
            false => match self.parent.next(pager, self.arena.as_ref()) {
                Err(e) => {
                    let _ = pager.rollback();
                    Err(e)
                }
                ok => ok,
            },
        }
    }
}

use crate::SqliteMaster;
use crate::backend::executor::Row;
use crate::backend::executor::context::ExecCtx;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

use super::plan::Plan;

#[derive(Debug)]
pub struct PreparedPlan<V: Vfs> {
    pub parent: Plan<V>,
    pub arena: ExprArena,
}
impl<V: Vfs> PreparedPlan<V> {
    pub fn new(parent: Plan<V>, arena: ExprArena) -> Self {
        Self { parent, arena }
    }
    pub fn next(
        &mut self,
        pager: &mut Pager<V>,
        master: &SqliteMaster,
    ) -> Result<Option<Row>, SqliteError> {
        if pager.start_transaction() {
            let parent_res = {
                let mut ctx = ExecCtx::new(pager, master, &self.arena);
                self.parent.next(&mut ctx)
            };
            match parent_res {
                Ok(_) => pager.commit()?,
                _ => pager.rollback()?,
            }
            parent_res
        } else {
            let parent_res = {
                let mut ctx = ExecCtx::new(pager, master, &self.arena);
                self.parent.next(&mut ctx)
            };
            match parent_res {
                Err(e) => {
                    let _ = pager.rollback();
                    Err(e)
                }
                ok => ok,
            }
        }
    }
}

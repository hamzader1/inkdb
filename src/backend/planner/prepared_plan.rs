use crate::Master;
use crate::backend::executor::Row;
use crate::backend::executor::context::ExecCtx;
use crate::errors::InkError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

use super::plan::Plan;

#[derive(Debug)]
pub struct PreparedPlan<V: Vfs> {
    pub parent: Plan<V>,
    pub arena: ExprArena,
    pub statement_table: Option<String>,
}
impl<V: Vfs> PreparedPlan<V> {
    pub fn table_name(&self) -> Option<&str> {
        self.statement_table.as_deref()
    }

    pub fn with_table(mut self, table: &str) -> Self {
        self.statement_table = Some(table.to_string());
        self
    }

    pub fn new(parent: Plan<V>, arena: ExprArena) -> Self {
        Self {
            parent,
            arena,
            statement_table: None,
        }
    }
    pub fn next(
        &mut self,
        pager: &mut Pager<V>,
        master: &mut Master,
    ) -> Result<Option<Row>, InkError> {
        if pager.start_transaction() {
            let parent_res = {
                let mut ctx = ExecCtx::new(
                    pager,
                    master,
                    &self.arena,
                    self.statement_table.as_deref(),
                );
                self.parent.next(&mut ctx)
            };
            match parent_res {
                Ok(_) => pager.commit()?,
                _ => {
                    pager.rollback()?;
                    master.is_dirty = true;
                }
            }
            parent_res
        } else {
            let parent_res = {
                let mut ctx = ExecCtx::new(
                    pager,
                    master,
                    &self.arena,
                    self.statement_table.as_deref(),
                );
                self.parent.next(&mut ctx)
            };
            match parent_res {
                Err(e) => {
                    pager.rollback()?;
                    master.is_dirty = true;
                    Err(e)
                }
                ok => ok,
            }
        }
    }
}

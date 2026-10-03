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
    parent: Plan<V>,
    arena: ExprArena,
    statement_table: Option<String>,
}
impl<V: Vfs> PreparedPlan<V> {
    pub fn parent(&self) -> &Plan<V> {
        &self.parent
    }

    pub fn arena(&self) -> &ExprArena {
        &self.arena
    }

    pub fn table_name(&self) -> Option<&str> {
        self.statement_table.as_deref()
    }

    pub(crate) fn with_table(mut self, table: &str) -> Self {
        self.statement_table = Some(table.to_string());
        self
    }

    pub(crate) fn take_statement_table(&mut self) -> Option<String> {
        self.statement_table.take()
    }

    pub(crate) fn set_statement_table(&mut self, table: Option<String>) {
        self.statement_table = table;
    }

    pub(crate) fn into_parts(self) -> (Plan<V>, ExprArena, Option<String>) {
        let Self {
            parent,
            arena,
            statement_table,
        } = self;
        (parent, arena, statement_table)
    }

    pub(crate) fn new(parent: Plan<V>, arena: ExprArena) -> Self {
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
                let mut ctx =
                    ExecCtx::new(pager, master, &self.arena, self.statement_table.as_deref());
                self.parent.next(&mut ctx)
            };
            match parent_res {
                Ok(_) => pager.commit()?,
                _ => {
                    pager.rollback()?;
                    master.mark_dirty();
                }
            }
            parent_res
        } else {
            let parent_res = {
                let mut ctx =
                    ExecCtx::new(pager, master, &self.arena, self.statement_table.as_deref());
                self.parent.next(&mut ctx)
            };
            match parent_res {
                Err(e) => {
                    pager.rollback()?;
                    master.mark_dirty();
                    Err(e)
                }
                ok => ok,
            }
        }
    }
}

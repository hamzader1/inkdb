use crate::Master;
use crate::backend::executor::Row;
use crate::backend::executor::context::ExecCtx;
use crate::errors::InkError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

use super::plan::Plan;

#[derive(Debug)]
pub enum PreparedPlan<V: Vfs> {
    AutoCommit {
        parent: Plan<V>,
        arena: ExprArena,
        statement_table: Option<String>,
    },
    Direct {
        parent: Plan<V>,
        arena: ExprArena,
        statement_table: Option<String>,
    },
}

impl<V: Vfs> PreparedPlan<V> {
    pub(crate) fn new(parent: Plan<V>, arena: ExprArena) -> Self {
        Self::AutoCommit {
            parent,
            arena,
            statement_table: None,
        }
    }

    pub(crate) fn direct(parent: Plan<V>, arena: ExprArena) -> Self {
        Self::Direct {
            parent,
            arena,
            statement_table: None,
        }
    }

    pub fn parent(&self) -> &Plan<V> {
        match self {
            Self::AutoCommit { parent, .. } | Self::Direct { parent, .. } => parent,
        }
    }

    pub fn arena(&self) -> &ExprArena {
        match self {
            Self::AutoCommit { arena, .. } | Self::Direct { arena, .. } => arena,
        }
    }

    pub fn table_name(&self) -> Option<&str> {
        match self {
            Self::AutoCommit {
                statement_table, ..
            }
            | Self::Direct {
                statement_table, ..
            } => statement_table.as_deref(),
        }
    }

    pub(crate) fn with_table(mut self, table: &str) -> Self {
        match &mut self {
            Self::AutoCommit {
                statement_table, ..
            }
            | Self::Direct {
                statement_table, ..
            } => *statement_table = Some(table.to_string()),
        }
        self
    }

    pub(crate) fn set_statement_table(&mut self, table: Option<String>) {
        match self {
            Self::AutoCommit {
                statement_table, ..
            }
            | Self::Direct {
                statement_table, ..
            } => *statement_table = table,
        }
    }

    pub(crate) fn into_parts(self) -> (Plan<V>, ExprArena, Option<String>) {
        match self {
            Self::AutoCommit {
                parent,
                arena,
                statement_table,
            }
            | Self::Direct {
                parent,
                arena,
                statement_table,
            } => (parent, arena, statement_table),
        }
    }

    pub fn next(
        &mut self,
        pager: &mut Pager<V>,
        master: &mut Master,
    ) -> Result<Option<Row>, InkError> {
        match self {
            Self::Direct {
                parent,
                arena,
                statement_table,
            } => {
                let mut ctx = ExecCtx::new(pager, master, arena, statement_table.as_deref());
                parent.next(&mut ctx)
            }
            Self::AutoCommit {
                parent,
                arena,
                statement_table,
            } => {
                if pager.start_transaction() {
                    let parent_res = {
                        let mut ctx =
                            ExecCtx::new(pager, master, arena, statement_table.as_deref());
                        parent.next(&mut ctx)
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
                            ExecCtx::new(pager, master, arena, statement_table.as_deref());
                        parent.next(&mut ctx)
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
    }
}

use crate::Master;
use crate::backend::executor::Row;
use crate::backend::executor::context::ExecCtx;
use crate::errors::InkError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

use super::plan::Plan;

/// A plan that is ready to run, with the arena its expressions live in.
///
/// The difference between the two variants is who owns the transaction. An
/// autocommit statement starts one, runs, and commits or rolls back by itself,
/// while a direct one runs inside a transaction that is already open.
#[derive(Debug)]
pub enum PreparedPlan<V: Vfs> {
    /// A statement that owns its transaction.
    AutoCommit {
        parent: Plan<V>,
        arena: ExprArena,
        statement_table: Option<String>,
    },
    /// A statement running inside a transaction someone else opened.
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

    /// The root of the plan.
    pub fn parent(&self) -> &Plan<V> {
        match self {
            Self::AutoCommit { parent, .. } | Self::Direct { parent, .. } => parent,
        }
    }

    /// The arena the plan expressions are held in.
    pub fn arena(&self) -> &ExprArena {
        match self {
            Self::AutoCommit { arena, .. } | Self::Direct { arena, .. } => arena,
        }
    }

    /// The statement's table, used in CHECK constraint errors.
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

    /// Note which table the statement is about.
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

    /// Set the table name for a wrapped statement such as `EXPLAIN`.
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

    /// Take the plan apart, for wrapping it in another plan.
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

    /// Run the plan.
    ///
    /// An autocommit plan starts a transaction, walks the plan, and then commits
    /// or rolls back. If the plan failed it is rolled back and the schema is
    /// marked stale, since a rolled back create or drop left the file as it was.
    /// A direct plan just runs, leaving the transaction to whoever opened it.
    pub fn next(
        &mut self,
        pager: &mut Pager<V>,
        master: &mut Master,
    ) -> Result<Option<Row>, InkError> {
        // LIMITATION: The current pager API supports only three operations: begin, rollback,
        // and commit. Single statement rollback is not supported yet. For example, if
        // a transaction contains five queries and the first four succeed but the last
        // one fails, the database rolls back the entire transaction, including the
        // four queries that succeeded.
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

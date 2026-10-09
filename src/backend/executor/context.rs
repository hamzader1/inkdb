use crate::Master;
use crate::pager::pager::Pager;
use crate::schema::Table;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

/// What an operator is given to do its work: the pager, the schema, the arena
/// its expressions live in, and the table the statement is about.
pub struct ExecCtx<'a, V: Vfs> {
    pub(crate) pager: &'a mut Pager<V>,
    pub(crate) master: &'a mut Master,
    pub(crate) arena: &'a ExprArena,
    pub(crate) statement_table: Option<&'a str>,
}

impl<'a, V: Vfs> ExecCtx<'a, V> {
    pub fn new(
        pager: &'a mut Pager<V>,
        master: &'a mut Master,
        arena: &'a ExprArena,
        statement_table: Option<&'a str>,
    ) -> Self {
        Self {
            pager,
            master,
            arena,
            statement_table,
        }
    }

    /// The table the statement is about.
    ///
    /// The name is looked up again here rather than kept, because a plan is built
    /// and run in one step and the schema cannot change in between.
    pub fn table(&self) -> Option<&Table> {
        self.statement_table
            .and_then(|name| self.master.table(name))
    }
}

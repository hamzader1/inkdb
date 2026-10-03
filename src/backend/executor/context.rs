use crate::Master;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

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

    pub fn table(&self) -> Option<&crate::schema::Table> {
        self.statement_table
            .and_then(|name| self.master.table(name))
    }
}

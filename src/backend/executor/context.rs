use crate::SqliteMaster;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;

pub struct ExecCtx<'a, V: Vfs> {
    pub pager: &'a mut Pager<V>,
    pub master: &'a SqliteMaster,
    pub arena: &'a ExprArena,
}

impl<'a, V: Vfs> ExecCtx<'a, V> {
    pub fn new(pager: &'a mut Pager<V>, master: &'a SqliteMaster, arena: &'a ExprArena) -> Self {
        Self {
            pager,
            master,
            arena,
        }
    }
}

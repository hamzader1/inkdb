use crate::backend::executor::Row;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;

use super::plan::Plan;

#[derive(Debug)]
pub struct Arena<F: SqliteFile> {
    pub parent: Plan<F>,
    pub arena: Option<ExprArena>,
}
impl<F: SqliteFile> Arena<F> {
    pub fn new(parent: Plan<F>, arena: Option<ExprArena>) -> Self {
        Self { parent, arena }
    }
    pub fn next(&mut self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        match pager.in_transaction() {
            false => {
                pager.start_transaction();
                let parent_res = self.parent.next(pager, self.arena.as_ref());
                if parent_res.is_ok() {
                    pager.commit()?;
                }
                parent_res
            }
            true => self.parent.next(pager, self.arena.as_ref()),
        }
    }
}

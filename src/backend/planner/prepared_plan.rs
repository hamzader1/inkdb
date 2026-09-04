use crate::backend::executor::Row;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;

use super::plan::Plan;

#[derive(Debug)]
pub struct PreparedPlan<F: SqliteFile> {
    pub parent: Plan<F>,
    pub arena: Option<ExprArena>,
}
impl<F: SqliteFile> PreparedPlan<F> {
    pub fn new(parent: Plan<F>, arena: Option<ExprArena>) -> Self {
        Self { parent, arena }
    }
    pub fn next(&mut self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        match pager.start_transaction() {
            true => {
                let parent_res = self.parent.next(pager, self.arena.as_ref());
                match parent_res {
                    Ok(_) => pager.commit()?,
                    _ => pager.rollback()?,
                }
                parent_res
            }
            false => self.parent.next(pager, self.arena.as_ref()),
        }
    }
}

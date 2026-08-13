use super::super::executor::{project::Project, scan::TableScan};
use crate::backend::analyze::ResolvedQuery;
use crate::backend::executor::{Row};
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::Arena;
use crate::vfs::file::SqliteFile;

#[derive(Debug)]
pub enum Plan {
    TableScan(TableScan),
    Project(Project),
}
impl Plan {
    pub fn create_plan(resolved_query: ResolvedQuery, pager: &mut Pager) -> Self {
        let table = Self::TableScan(TableScan::new(resolved_query.root_page, pager));

        /*
         *
         *  TODO: missing where clause
         *
         *
         */

        Self::Project(Project::new(
            Box::new(table),
            resolved_query.arena.clone(),
            resolved_query.columns.clone(),
        ))
    }
}

impl Plan {
    pub fn next(&mut self, pager: &mut Pager) -> Result<Option<Row>, SqliteError> {
        match self {
            Self::TableScan(t) => t.next(pager),
            Self::Project(p) => p.next(pager),
        }
    }
}

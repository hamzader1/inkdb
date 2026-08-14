use super::super::executor::{project::Project, scan::TableScan};
use crate::backend::analyze::ResolvedQuery;
use crate::backend::executor::Row;
use crate::backend::executor::filter::Filter;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::Arena;
use crate::vfs::file::SqliteFile;

#[derive(Debug)]
pub enum Plan {
    TableScan(TableScan),
    Filter(Filter),
    Project(Project),
}
impl Plan {
    pub fn create_plan(resolved_query: ResolvedQuery, pager: &mut Pager) -> Self {
        let mut child = Self::TableScan(TableScan::new(resolved_query.root_page, pager));
        if let Some(predict) = resolved_query.where_clause {
            child = Self::Filter(Filter::new(
                Box::new(child),
                resolved_query.arena.clone(),
                predict,
            ));
        }
        Self::Project(Project::new(
            Box::new(child),
            resolved_query.arena.clone(),
            resolved_query.columns.clone(),
        ))
    }
}

impl Plan {
    pub fn next(&mut self, pager: &mut Pager) -> Result<Option<Row>, SqliteError> {
        match self {
            Self::TableScan(t) => t.next(pager),
            Self::Filter(f) => f.next(pager),
            Self::Project(p) => p.next(pager),
        }
    }
}

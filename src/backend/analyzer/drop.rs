use crate::{
    InkResult,
    backend::{
        analyzer::{Analyze, ResolvedQuery},
        planner::plan::index_roots,
    },
    errors::InkError,
    sql::ast::{DropIndexStmt, DropTableStmt},
};

use super::{ResolvedDropIndexQuery, ResolvedDropTableQuery};

impl<'a> Analyze<'a> {
    pub(crate) fn analyze_drop_tbl(&self, stmt: DropTableStmt) -> InkResult<ResolvedQuery> {
        let tbl = self.get_non_master_table(&stmt.tbl_name)?;
        let indexes = index_roots(self.master, tbl.name())?;
        Ok(ResolvedQuery::DropTblQuery(ResolvedDropTableQuery {
            root_page: tbl.root_page(),
            tbl_name: tbl.name().clone(),
            indexes,
        }))
    }

    pub(crate) fn analyze_drop_index(&self, stmt: DropIndexStmt) -> InkResult<ResolvedQuery> {
        let index = self
            .master
            .index(&stmt.index_name)
            .ok_or_else(|| InkError::IndexNotFound(stmt.index_name.clone()))?;
        Ok(ResolvedQuery::DropIndexQuery(ResolvedDropIndexQuery {
            index_name: index.name().clone(),
            root_page: index.root_page(),
        }))
    }
}

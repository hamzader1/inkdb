use crate::{
    InkResult,
    backend::{
        analyzer::{Analyze, ResolvedQuery},
        planner::plan::index_roots,
    },
    sql::ast::DropTableStmt,
};

use super::ResolvedDropTableQuery;

impl<'a> Analyze<'a> {
    pub fn analyze_drop_tbl(&self, stmt: DropTableStmt) -> InkResult<ResolvedQuery> {
        let tbl = self.get_non_master_table(&stmt.tbl_name)?;
        let indexes = index_roots(self.master, &stmt.tbl_name)?;
        Ok(ResolvedQuery::DropTblQuery(ResolvedDropTableQuery {
            root_page: tbl.root_page,
            tbl_name: stmt.tbl_name,
            indexes,
        }))
    }
}

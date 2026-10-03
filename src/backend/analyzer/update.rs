use crate::Master;
use crate::backend::analyzer::bind::FastBind;
use crate::backend::analyzer::{Analyze, ResolvedQuery};
use crate::errors::InkError;
use crate::sql::ast::UpdateStmt;

use super::ResolvedUpdateQuery;

impl<'a> Analyze<'a> {
    pub(crate) fn analyze_update_stmt(&self, update_stmt: UpdateStmt) -> Result<ResolvedQuery, InkError> {
        let UpdateStmt {
            table_name,
            columns,
            where_clause,
            mut arena,
        } = update_stmt;
        let table = self.get_table(&table_name)?;
        let root_page = table.root_page();
        for (col, expr) in columns.iter() {
            Self::fast_bind(table, *col, &mut arena)?;
            Self::fast_bind(table, *expr, &mut arena)?;
        }
        if let Some(predicate) = where_clause {
            Self::fast_bind(table, predicate, &mut arena)?;
        }
        Ok(ResolvedQuery::UpdateQuery(ResolvedUpdateQuery::new(
            table_name,
            root_page,
            columns.into(),
            where_clause,
            arena,
        )))
    }
}

use crate::Master;
use crate::backend::analyze::bind::FastBind;
use crate::backend::analyze::{Analyze, ResolvedQuery};
use crate::errors::InkError;
use crate::sql::ast::UpdateStmt;

use super::ResolvedUpdateQuery;

impl Analyze {
    pub fn analyze_update_stmt(
        update_stmt: UpdateStmt,
        master: &Master,
    ) -> Result<ResolvedQuery, InkError> {
        let UpdateStmt {
            table_name,
            columns,
            where_clause,
            mut arena,
        } = update_stmt;
        let table = Self::get_table(master, &table_name)?;
        let root_page = table.root_page;
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
            columns,
            where_clause,
        )))
    }
}

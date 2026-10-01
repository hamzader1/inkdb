use crate::backend::analyzer::ResolvedCreateIndexQuery;
use crate::errors::InkError;
use crate::sql::ast::{CreateIndex, CreateTableStmt};
use crate::{InkResult, Master};

use super::{Analyze, ResolvedCreateTableQuery, ResolvedQuery};

impl<'a> Analyze<'a> {
    pub(super) fn analyze_create_table_stmt(
        &self,
        stmt: CreateTableStmt,
    ) -> Result<ResolvedQuery, InkError> {
        if self.get_non_master_table(&stmt.name).is_ok() {
            return Err(InkError::TableAlreadyExists(stmt.name));
        }

        Ok(ResolvedQuery::CreateTableQuery(ResolvedCreateTableQuery {
            meta: stmt,
        }))
    }

    pub fn analyze_create_index_stmt(&self, stmt: CreateIndex) -> InkResult<ResolvedQuery> {
        let relation = self.get_non_master_table(&stmt.table)?;
        if self.master.indexes.contains_key(&stmt.name) {
            return Err(InkError::runtime(format!(
                "Index with name {} already exists",
                stmt.name
            )));
        }
        // single col only for now
        if stmt.columns.is_empty() {
            return Err(InkError::runtime(
                "Index can be created on empty column set",
            ));
        }
        let column_index = match relation.get_col_idx(&stmt.columns[0]) {
            Some(idx) => idx,
            None => return Err(InkError::UnknownColumn(stmt.columns[0].to_string())),
        };

        Ok(ResolvedQuery::CreateIndexQuery(ResolvedCreateIndexQuery {
            query: stmt.query,
            relation_root_page: relation.root_page,
            relation_name: relation.name.clone(),
            index_name: stmt.name,
            column_index,
            is_unique: stmt.unique,
        }))
    }
}

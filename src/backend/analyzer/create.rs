use crate::backend::analyzer::ResolvedCreateIndexQuery;
use crate::errors::InkError;
use crate::sql::ast::{CreateIndex, CreateTableStmt};
use crate::{Master, InkResult};

use super::{Analyze, ResolvedCreateTableQuery, ResolvedQuery};

impl Analyze {
    pub(super) fn analyze_create_table_stmt(
        stmt: CreateTableStmt,
        master: &Master,
    ) -> Result<ResolvedQuery, InkError> {
        if Self::get_non_master_table(master, &stmt.name).is_ok() {
            return Err(InkError::TableAlreadyExists(stmt.name));
        }

        Ok(ResolvedQuery::CreateTableQuery(ResolvedCreateTableQuery {
            meta: stmt,
        }))
    }

    pub fn analyze_create_index_stmt(
        stmt: CreateIndex,
        master: &Master,
    ) -> InkResult<ResolvedQuery> {
        let relation = Self::get_non_master_table(master, &stmt.table)?;
        if master.indexes.contains_key(&stmt.name) {
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
        }))
    }
}

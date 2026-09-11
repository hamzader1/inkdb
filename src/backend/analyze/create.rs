use crate::backend::analyze::ResolvedCreateIndexQuery;
use crate::errors::SqliteError;
use crate::sql::ast::{CreateIndex, CreateTable};
use crate::{SqliteMaster, SqliteResult};

use super::{Analyze, ResolvedCreateTableQuery, ResolvedQuery};

impl Analyze {
    pub(super) fn analyze_create_table_stmt(
        stmt: CreateTable,
        sqlite_master: &SqliteMaster,
    ) -> Result<ResolvedQuery, SqliteError> {
        if Self::get_table(sqlite_master, &stmt.name).is_ok() {
            return Err(SqliteError::TableAlreadyExists(stmt.name));
        }

        Ok(ResolvedQuery::CreateTableQuery(ResolvedCreateTableQuery {
            meta: stmt,
        }))
    }

    pub fn analyze_create_index_stmt(
        stmt: CreateIndex,
        sqlite_master: &SqliteMaster,
    ) -> SqliteResult<ResolvedQuery> {
        let relation = Self::get_table(sqlite_master, &stmt.table)?;
        if sqlite_master.indexes.contains_key(&stmt.name) {
            return Err(SqliteError::Runtime(format!(
                "Index with name {} already exists",
                stmt.name
            )));
        }
        // single col only for now
        if stmt.columns.is_empty() {
            return Err(SqliteError::Runtime(
                "Index can be created on empty column set".into(),
            ));
        }
        let column_index = match relation.get_col_idx(&stmt.columns[0]) {
            Some(idx) => idx,
            None => return Err(SqliteError::UnknownColumn(stmt.columns[0].to_string())),
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

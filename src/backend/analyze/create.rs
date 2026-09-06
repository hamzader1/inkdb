use crate::SqliteMaster;
use crate::errors::SqliteError;
use crate::sql::ast::CreateTable;

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
}

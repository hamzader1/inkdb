use crate::SqliteMaster;
use crate::errors::SqliteError;
use crate::format::page::PageNo;
use crate::record::Value;
use crate::schema::Table;
use crate::sql::ast::{Affinity, Ast, CreateTable, Expr, InsertStmt, SelectStmt};
use crate::sql::parser::ExprArena;
use crate::util::{sqlite_assert_one, sqlite_assert_with_corrupt_err};

use super::{Analyze, ResolvedCreateTableQuery, ResolvedQuery};

impl Analyze {
    pub(super) fn analyze_create_table_stmt(
        stmt: CreateTable,
        sqlite_master: &SqliteMaster,
    ) -> Result<ResolvedQuery, SqliteError> {
        if Self::get_table(sqlite_master, &stmt.name).is_ok() {
            return Err(SqliteError::RuntimeError(format!(
                "Table {} already exists",
                stmt.name
            )));
        }

        Ok(ResolvedQuery::CreateTableQuery(ResolvedCreateTableQuery {
            meta: stmt,
        }))
    }
}

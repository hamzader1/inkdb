use crate::backend::analyze::{ResolvedDeleteQuery, ResolvedTruncateTableQuery};
use crate::sql::ast::DeleteStmt;
use crate::{SqliteMaster, SqliteResult};

use super::{Analyze, ResolvedQuery};

impl Analyze {
    pub fn analyze_delete_stmt(
        mut stmt: DeleteStmt,
        sqlite_master: &SqliteMaster,
    ) -> SqliteResult<ResolvedQuery> {
        let table = Self::get_table(sqlite_master, &stmt.table_name)?;
        if let Some(predicate) = stmt.where_clause {
            let arena = stmt
                .arena
                .as_mut()
                .expect("Where clause without an arena parent is not allowed");
            Self::fast_bind(table, predicate, arena)?;
        }
        // BASIC, Sql ( "DELETE FROM t" )
        else {
            return Ok(ResolvedQuery::TruncateTable(ResolvedTruncateTableQuery {
                table_name: table.name.clone(),
                root_page: table.root_page,
            }));
        }
        Ok(ResolvedQuery::DeleteQuery(ResolvedDeleteQuery {
            table_name: table.name.clone(),
            root_page: table.root_page,
            arena: stmt.arena,
            where_clause: stmt.where_clause,
        }))
    }
}

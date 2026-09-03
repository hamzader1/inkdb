use crate::backend::analyze::ResolvedDeleteQuery;
use crate::sql::ast::DeleteStmt;
use crate::{SqliteMaster, SqliteResult};

use super::{Analyze, ResolvedQuery};

impl Analyze {
    pub fn analyze_delete_stmt(
        mut stmt: DeleteStmt,
        sqlite_master: &SqliteMaster,
    ) -> SqliteResult<ResolvedQuery> {
        let DeleteStmt {
            table_name,
            arena,
            where_clause,
        } = &mut stmt;
        let table = Self::get_table(sqlite_master, table_name)?;
        if let Some(predict) = stmt.where_clause {
            let arena = stmt.arena.as_mut().expect(
                "
                    Where clause without an arena parent is now allowed
                ",
            );
            Self::fast_bind(table, predict, stmt.arena.as_mut().unwrap())?;
        }
        Ok(ResolvedQuery::DeleteQuery(ResolvedDeleteQuery {
            root_page: table.root_page,
            arena: stmt.arena,
            where_clause: stmt.where_clause,
        }))
    }
}

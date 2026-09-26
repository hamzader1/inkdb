use crate::backend::analyze::{ResolvedDeleteQuery, ResolvedTruncateTableQuery};
use crate::errors::SqliteError;
use crate::sql::ast::DeleteStmt;
use crate::{SqliteMaster, SqliteResult};

use super::{Analyze, IndexMetadata, ResolvedQuery};

impl Analyze {
    // TODO: Expand and add optimizer
    pub fn analyze_delete_stmt(
        mut stmt: DeleteStmt,
        sqlite_master: &SqliteMaster,
    ) -> SqliteResult<ResolvedQuery> {
        // let DeleteStmt { table_name, .. } = &mut stmt;
        let table = Self::get_table(sqlite_master, &stmt.table_name)?;
        if let Some(predicate) = stmt.where_clause {
            let arena = stmt.arena.as_mut().expect(
                "
                    Where clause without an arena parent is now allowed
                ",
            );
            Self::fast_bind(table, predicate, arena)?;
        }
        // BASIC, Sql ( "DELETE FROM t" )
        else {
            let mut indexes = Vec::new();
            for index in sqlite_master.indexes.values() {
                if index.table.eq_ignore_ascii_case(&stmt.table_name) {
                    indexes.push(index.root_page)
                }
            }
            return Ok(ResolvedQuery::TruncateTable(ResolvedTruncateTableQuery {
                root_page: table.root_page,
                indexes: if indexes.is_empty() {
                    None
                } else {
                    Some(indexes)
                },
            }));
        }
        let mut indexes = Vec::new();
        for index in sqlite_master.indexes.values() {
            if index.table == table.name {
                let col_idx = table
                    .get_col_idx(&index.columns[0])
                    .ok_or(SqliteError::runtime(format!(
                        "
                            Column {} does not exist in table {}
                            ",
                        index.columns[0], stmt.table_name
                    )))?;
                indexes.push(IndexMetadata {
                    index_root_page: index.root_page,
                    col_idx,
                    is_unique: index.unique,
                });
            }
        }
        Ok(ResolvedQuery::DeleteQuery(ResolvedDeleteQuery {
            table_name: table.name.clone(),
            root_page: table.root_page,
            indexes: if indexes.is_empty() {
                None
            } else {
                Some(indexes)
            },
            arena: stmt.arena,
            where_clause: stmt.where_clause,
        }))
    }
}

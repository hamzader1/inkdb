use crate::SqliteMaster;
use crate::backend::analyze::IndexMetadata;
use crate::errors::SqliteError;
use crate::sql::ast::{Affinity, InsertStmt};
use crate::util::sqlite_assert_with_runtime_err;

use super::{Analyze, ResolvedInsertQuery, ResolvedQuery};

impl Analyze {
    pub fn analyze_insert_stmt(
        stmt: InsertStmt,
        sqlite_master: &SqliteMaster,
    ) -> Result<ResolvedQuery, SqliteError> {
        let InsertStmt {
            table_name,
            columns,
            values,
        } = stmt;

        let table = Self::get_table(sqlite_master, &table_name)?;
        // case1: no columns (default for now)

        if columns.is_empty() {
            for inner_values in values.iter() {
                sqlite_assert_with_runtime_err(
                    inner_values.len() == table.columns.len(),
                    &format!(
                        "Column count mismatch: table has {} columns, but {} columns were provided",
                        table.columns.len(),
                        inner_values.len(),
                    ),
                )?;
                for (i, value) in inner_values.iter().enumerate() {
                    let sqlite_value_type = Affinity::from(value);
                    sqlite_assert_with_runtime_err(
                        sqlite_value_type == table.columns[i].affinity,
                        format!(
                            "Type mismatch on column '{}': table defines '{}' but the value has affinity '{}'",
                            table.columns[i].name, table.columns[i].affinity, sqlite_value_type
                        )
                        .as_str(),
                    )?;
                }
            }
        }

        let mut indexes: Option<Vec<IndexMetadata>> = None;
        for index in sqlite_master.indexes.values() {
            if index.table == table.name {
                if let Some(ref mut indexes) = indexes {
                    let col_idx =
                        table
                            .get_col_idx(&index.columns[0])
                            .ok_or(SqliteError::Runtime(format!(
                                "
                            Column {} does not exist in table {}
                            ",
                                &index.columns[0], table_name
                            )))?;
                    indexes.push(IndexMetadata {
                        index_root_page: index.root_page,
                        col_idx,
                        is_unique: index.unique,
                    });
                    continue;
                }
                indexes = Some(Vec::new());
            }
        }

        Ok(ResolvedQuery::InsertQuery(ResolvedInsertQuery {
            root_page: table.root_page,
            values,
            indexes,
            entry_hint: None,
        }))
    }
}

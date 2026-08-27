use crate::SqliteMaster;
use crate::errors::SqliteError;
use crate::sql::ast::{Affinity, InsertStmt};
use crate::util::sqlite_assert_with_corrupt_err;

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
                assert!(inner_values.len() == table.columns.len());
                for (i, value) in inner_values.iter().enumerate() {
                    let sqlite_value_type = Affinity::from(value);
                    sqlite_assert_with_corrupt_err(
                        sqlite_value_type == table.columns[i].affinity,
                        format!(
                            "Column '{}' has data type of '{}' but '{}' were given",
                            table.columns[i].name, table.columns[i].affinity, sqlite_value_type
                        )
                        .as_str(),
                    )?;
                }
            }
        }

        Ok(ResolvedQuery::InsertQuery(ResolvedInsertQuery {
            root: table.root_page,
            values,
            entry_hint: None,
        }))
    }
}

use crate::Master;
use crate::errors::InkError;
use crate::sql::ast::{Affinity, InsertStmt};
use crate::util::assert_with_runtime_err;

use super::{Analyze, ResolvedInsertQuery, ResolvedQuery};

impl Analyze {
    pub fn analyze_insert_stmt(
        stmt: InsertStmt,
        master: &Master,
    ) -> Result<ResolvedQuery, InkError> {
        let InsertStmt {
            table_name,
            columns,
            values,
        } = stmt;

        assert_with_runtime_err(!table_name.eq_ignore_ascii_case("master"), || {
            "table master may not be modified".into()
        })?;
        let table = Self::get_non_master_table(master, &table_name)?;
        // case1: no columns (default for now)

        if columns.is_empty() {
            for inner_values in values.iter() {
                assert_with_runtime_err(inner_values.len() == table.columns.len(), || {
                    format!(
                        "Column count mismatch: table has {} columns, but {} columns were provided",
                        table.columns.len(),
                        inner_values.len(),
                    )
                })?;
                for (i, value) in inner_values.iter().enumerate() {
                    let value_type = Affinity::try_from(value)?;
                    assert_with_runtime_err(
                        value_type == table.columns[i].affinity,
                        || {
                            format!(
                                "Type mismatch on column '{}': table defines '{}' but the value has affinity '{}'",
                                table.columns[i].name, table.columns[i].affinity, value_type
                            )
                        },
                    )?;
                }
            }
        }

        Ok(ResolvedQuery::InsertQuery(ResolvedInsertQuery {
            table_name: table.name.clone(),
            root_page: table.root_page,
            values,
        }))
    }
}

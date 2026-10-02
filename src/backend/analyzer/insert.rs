use crate::Master;
use crate::backend::executor::Row;
use crate::backend::executor::eval::{Eval, render_expr};
use crate::errors::InkError;
use crate::sql::ast::{Affinity, InsertStmt};
use crate::util::assert_with_runtime_err;

use super::{Analyze, ResolvedInsertQuery, ResolvedQuery};

impl<'a> Analyze<'a> {
    pub fn analyze_insert_stmt(&self, stmt: InsertStmt) -> Result<ResolvedQuery, InkError> {
        let InsertStmt {
            table_name,
            columns,
            values,
            arena,
        } = stmt;

        assert_with_runtime_err(!table_name.eq_ignore_ascii_case("master"), || {
            "table master may not be modified".into()
        })?;
        let table = self.get_non_master_table(&table_name)?;
        let mut evalued_rows = Vec::new();
        let mut evalued_vals = Vec::new();
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
                for (i, idx) in inner_values.iter().enumerate() {
                    let value = Eval::eval(&arena, *idx, None)?;
                    let value_type = Affinity::try_from(&value)?;
                    assert_with_runtime_err(value_type == table.columns[i].affinity, || {
                        format!(
                            "Type mismatch on column '{}': table defines '{}' but the value has affinity '{}'",
                            table.columns[i].name, table.columns[i].affinity, value_type
                        )
                    })?;
                    evalued_vals.push(value);
                }
                for cst in table.tbl_constraits.iter() {
                    let bool_res =
                        Eval::eval(&table.tbl_arena, *cst, Some(&evalued_vals))?.to_bool();
                    assert_with_runtime_err(bool_res, || {
                        format!(
                            "CHECK constraint failed: {}",
                            render_expr(&table.tbl_arena, *cst, Some(table))
                        )
                    })?;
                }
                evalued_rows.push(std::mem::take(&mut evalued_vals));
            }
        }

        Ok(ResolvedQuery::InsertQuery(ResolvedInsertQuery {
            table_name,
            root_page: table.root_page,
            values: evalued_rows,
        }))
    }
}

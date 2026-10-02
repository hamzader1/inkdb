use crate::InkResult;
use crate::backend::executor::eval::{Eval, render_expr};
use crate::errors::InkError;
use crate::record::Value;
use crate::schema::Table;
use crate::sql::ast::{Affinity, Column, Constraint, DefaultValue, InsertStmt};
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

        let table = self.get_non_master_table(&table_name)?;
        let mut evalued_rows = Vec::new();
        let mut evalued_vals = Vec::new();

        let columns_list = {
            let mut columns_list = Vec::new();
            if columns.is_empty() {
                for (i, column) in table.columns.iter().enumerate() {
                    columns_list.push((i, i, column));
                }
            } else {
                for (i, column) in table.columns.iter().enumerate() {
                    for (j, col_name) in columns.iter().enumerate() {
                        if col_name.eq_ignore_ascii_case(&column.name) {
                            columns_list.push((i, j, column));
                        }
                    }
                }
                assert_columns_resolved(table, &columns, columns_list.len())?;
                columns_list.sort_by_key(|entry| entry.0);
            }
            columns_list
        };

        let mut j = 0;
        for inner_values in values.iter() {
            assert_value_count(inner_values.len(), columns_list.len())?;
            for i in 0..table.columns.len() {
                let mapped = columns_list
                    .get(j)
                    .filter(|(k, _, _)| j < inner_values.len() && *k == i);
                let Some(&(_, delta, _)) = mapped else {
                    // if let Some(idx) = table.has_integer_primary_key() {
                    //     
                    // }
                    handle_missing(&table.columns[i], &table.name, &mut evalued_vals)?;
                    continue;
                };

                let value = Eval::eval(&arena, inner_values[delta], None)?;
                if matches!(value, Value::Null) {
                    assert_not_null(&table.columns[i], &table.name)?;
                } else {
                    let value_type = Affinity::try_from(&value)?;
                    assert_with_runtime_err(value_type == table.columns[i].affinity, || {
                        format!(
                            "Type mismatch on column '{}': table defines '{}' but the value has affinity '{}'",
                            table.columns[i].name, table.columns[i].affinity, value_type
                        )
                    })?;
                }
                evalued_vals.push(value);
                j += 1;
            }
            for cst in table.tbl_constraits.iter() {
                let bool_res = Eval::eval(&table.tbl_arena, *cst, Some(&evalued_vals))?.to_bool();
                assert_with_runtime_err(bool_res, || {
                    format!(
                        "CHECK constraint failed: {}",
                        render_expr(&table.tbl_arena, *cst, Some(table))
                    )
                })?;
            }
            evalued_rows.push(std::mem::take(&mut evalued_vals));
            j = 0;
        }

        Ok(ResolvedQuery::InsertQuery(ResolvedInsertQuery {
            table_name,
            root_page: table.root_page,
            values: evalued_rows,
        }))
    }
}

fn assert_value_count(provided: usize, expected: usize) -> InkResult<()> {
    assert_with_runtime_err(provided == expected, || {
        format!("{provided} values for {expected} columns")
    })
}

fn assert_columns_resolved(table: &Table, columns: &[String], resolved: usize) -> InkResult<()> {
    if resolved == columns.len() {
        return Ok(());
    }
    let unknown = columns.iter().find(|name| {
        !table
            .columns
            .iter()
            .any(|column| column.name.eq_ignore_ascii_case(name))
    });
    match unknown {
        Some(name) => Err(InkError::runtime(format!(
            "table {} has no column named {}",
            table.name, name
        ))),
        None => Err(InkError::runtime(format!(
            "INSERT names {} columns but only {} of them match {}",
            columns.len(),
            resolved,
            table.name
        ))),
    }
}

fn assert_not_null(col: &Column, table_name: &str) -> InkResult<()> {
    /*
     * Error if
     * the column has NonNull constrait.
     */
    assert_with_runtime_err(
        !col.constraints
            .as_ref()
            .is_some_and(|csts| csts.contains(&Constraint::NotNull)),
        || format!("NOT NULL constraint failed: {}.{}", table_name, col.name),
    )
}

fn handle_missing(col: &Column, table_name: &str, out: &mut Vec<Value>) -> InkResult<()> {
    if let Some(ref default) = col.default {
        assert!(matches!(default, DefaultValue::Val(_)));
        let DefaultValue::Val(value) = default else {
            unreachable!()
        };
        out.push(value.clone());
        return Ok(());
    };
    assert_not_null(col, table_name)?;

    /*
     * if there is no NonNull constrait but we do have default.
     * evaluate default if not already so.
     * and make it as value
     *
     * Default are ignored for now
     */
    out.push(Value::Null);
    Ok(())
}

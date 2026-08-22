use crate::SqliteMaster;
use crate::errors::SqliteError;
use crate::pager::pager::PageNo;

use crate::record::Value;
use crate::schema::Table;
use crate::sql::ast::{Affinity, Ast, Expr, InsertStmt, SelectStmt};
use crate::sql::parser::ExprArena;
use crate::util::{sqlite_assert_one, sqlite_assert_with_corrupt_err};

use super::Analyze;

impl Analyze {
    pub(super) fn slow_bind(
        table: &Table,
        idx: usize,
        arena: &mut ExprArena,
        new: &mut Vec<Expr>,
        map: &mut Vec<usize>,
        new_cols: &mut Vec<usize>,
    ) -> Result<(), SqliteError> {
        let expr = &arena.nodes[idx];
        match expr {
            Expr::Identifier(col_name) => match table.get_col_idx(&col_name.to_lowercase()) {
                Some(col_idx) => {
                    new.push(Expr::ColumnRef(col_idx));
                    map[idx] = new.len() - 1;
                    return Ok(());
                }
                _ => {
                    return Err(SqliteError::RuntimeError(format!(
                        "Column {} does not exists",
                        col_name
                    )));
                }
            },
            Expr::Star => {
                for i in 0..table.get_cols_len() {
                    new.push(Expr::ColumnRef(i));
                    new_cols.push(new.len() - 1);
                }
                map[idx] = new.len() - 1;
            }
            Expr::Number(_) | Expr::Float(_) | Expr::Bool(_) | Expr::StringLitteral(_) => {
                new.push(expr.clone());
                map[idx] = new.len() - 1;
                return Ok(());
            }
            Expr::Add(left, right)
            | Expr::Substract(left, right)
            | Expr::Multiply(left, right)
            | Expr::Devide(left, right) => {
                let left = *left;
                let right = *right;

                let node = expr.clone();
                Self::slow_bind(table, left, arena, new, map, new_cols)?;
                Self::slow_bind(table, right, arena, new, map, new_cols)?;
                new.push(Expr::remap_l_r(&node, map[left], map[right]));
                map[idx] = new.len() - 1;
            }
            Expr::Neg(i) => {
                let child = *i;
                Self::slow_bind(table, child, arena, new, map, new_cols)?;
                new.push(Expr::Neg(map[child]));
                map[idx] = new.len() - 1;
            }
            Expr::Not(i) => {
                let child = *i;
                Self::slow_bind(table, child, arena, new, map, new_cols)?;
                new.push(Expr::Not(map[child]));
                map[idx] = new.len() - 1;
            }
            Expr::And { left, right } => {
                let left = *left;
                let right = *right;

                let node = expr.clone();
                Self::slow_bind(table, left, arena, new, map, new_cols)?;
                Self::slow_bind(table, right, arena, new, map, new_cols)?;
                new.push(Expr::remap_l_r(&node, map[left], map[right]));
                map[idx] = new.len() - 1;
            }
            Expr::Or { left, right } => {
                let left = *left;
                let right = *right;

                let node = expr.clone();
                Self::slow_bind(table, left, arena, new, map, new_cols)?;
                Self::slow_bind(table, right, arena, new, map, new_cols)?;
                new.push(Expr::remap_l_r(&node, map[left], map[right]));
                map[idx] = new.len() - 1;
            }
            Expr::BinaryOp { left, op, right } => {
                let left = *left;
                let right = *right;

                let node = expr.clone();
                Self::slow_bind(table, left, arena, new, map, new_cols)?;
                Self::slow_bind(table, right, arena, new, map, new_cols)?;
                new.push(Expr::remap_l_r(&node, map[left], map[right]));
                map[idx] = new.len() - 1;
            }
            _ => {
                return Err(SqliteError::RuntimeError(
                    "Expression is not allowed in select statement list".into(),
                ));
            }
        }
        Ok(())
    }

    // General purpose
    pub(super) fn fast_bind(
        table: &Table,
        idx: usize,
        arena: &mut ExprArena,
    ) -> Result<(), SqliteError> {
        let expr = &arena.nodes[idx];
        match expr {
            Expr::Identifier(col_name) => match table.get_col_idx(&col_name.to_lowercase()) {
                Some(col_idx) => {
                    arena.nodes[idx] = Expr::ColumnRef(col_idx);
                    return Ok(());
                }
                _ => {
                    return Err(SqliteError::RuntimeError(format!(
                        "Column {} does not exists",
                        col_name
                    )));
                }
            },
            Expr::Number(_) | Expr::Float(_) | Expr::Bool(_) | Expr::StringLitteral(_) => {
                return Ok(());
            }
            Expr::Add(left, right) => {
                let left = *left;
                let right = *right;
                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Substract(left, right) => {
                let left = *left;
                let right = *right;
                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Multiply(left, right) => {
                let left = *left;
                let right = *right;
                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Devide(left, right) => {
                let left = *left;
                let right = *right;
                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Neg(i) => {
                Self::fast_bind(table, *i, arena)?;
            }
            Expr::Not(i) => {
                Self::fast_bind(table, *i, arena)?;
            }
            Expr::And { left, right } => {
                let left = *left;
                let right = *right;

                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Or { left, right } => {
                let left = *left;
                let right = *right;

                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::BinaryOp { left, op, right } => {
                let left = *left;
                let right = *right;

                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            _ => {
                return Err(SqliteError::RuntimeError(
                    "Expression is not allowed in select statement list".into(),
                ));
            }
        }
        Ok(())
    }
}

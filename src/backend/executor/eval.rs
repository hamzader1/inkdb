use std::borrow::Cow;

use crate::errors::InkError;
use crate::record::{TryAdd, TryDiv, TryMul, TrySub, Value};

use super::ColumnSource;
use crate::schema::Table;
use crate::sql::ast::{BinaryOperator, Expr};
use crate::sql::parser::ExprArena;

const MAX_RENDER_DEPTH: usize = 32;

pub fn render_expr(arena: &ExprArena, index: usize, table: Option<&Table>) -> String {
    render_expr_at(arena, index, table, 0)
}

fn render_expr_at(arena: &ExprArena, index: usize, table: Option<&Table>, depth: usize) -> String {
    if depth > MAX_RENDER_DEPTH {
        return format!("expr[{index}]");
    }
    let Some(expr) = arena.nodes.get(index) else {
        return format!("expr[{index}]");
    };
    let inner = depth + 1;
    match expr {
        Expr::Number(number) => number.to_string(),
        Expr::Float(float) => float.to_string(),
        Expr::Null => Expr::Null.to_string(),
        Expr::StringLitteral(text) => format!("'{text}'"),
        Expr::Bool(flag) => if *flag { "true" } else { "false" }.to_string(),
        Expr::Identifier(name) => name.clone(),
        Expr::ColumnRef(column) => match table.and_then(|table| table.get_col_name(*column)) {
            Some(column) => column.name.clone(),
            None => format!("column[{column}]"),
        },
        Expr::Star => "*".into(),
        Expr::Count { arg: Some(arg) } => {
            format!("count({})", render_expr_at(arena, *arg, table, inner))
        }
        Expr::Count { arg: None } => "count(*)".into(),
        Expr::Add(left, right) => format!(
            "({} + {})",
            render_expr_at(arena, *left, table, inner),
            render_expr_at(arena, *right, table, inner)
        ),
        Expr::Substract(left, right) => format!(
            "({} - {})",
            render_expr_at(arena, *left, table, inner),
            render_expr_at(arena, *right, table, inner)
        ),
        Expr::Multiply(left, right) => format!(
            "({} * {})",
            render_expr_at(arena, *left, table, inner),
            render_expr_at(arena, *right, table, inner)
        ),
        Expr::Devide(left, right) => format!(
            "({} / {})",
            render_expr_at(arena, *left, table, inner),
            render_expr_at(arena, *right, table, inner)
        ),
        Expr::Neg(child) => format!("-{}", render_expr_at(arena, *child, table, inner)),
        Expr::Not(child) => format!("NOT {}", render_expr_at(arena, *child, table, inner)),
        Expr::BinaryOp { left, op, right } => format!(
            "({} {} {})",
            render_expr_at(arena, *left, table, inner),
            render_operator(*op),
            render_expr_at(arena, *right, table, inner)
        ),
        Expr::And { left, right } => format!(
            "({} AND {})",
            render_expr_at(arena, *left, table, inner),
            render_expr_at(arena, *right, table, inner)
        ),
        Expr::Or { left, right } => format!(
            "({} OR {})",
            render_expr_at(arena, *left, table, inner),
            render_expr_at(arena, *right, table, inner)
        ),
    }
}

pub fn render_operator(op: BinaryOperator) -> &'static str {
    match op {
        BinaryOperator::Eq => "=",
        BinaryOperator::NotEq => "!=",
        BinaryOperator::Ge => ">=",
        BinaryOperator::Le => "<=",
        BinaryOperator::Gt => ">",
        BinaryOperator::Lt => "<",
    }
}

pub struct Eval;
impl Eval {
    pub fn eval<'a>(
        arena: &ExprArena,
        idx: usize,
        row: Option<&'a dyn ColumnSource>,
    ) -> Result<Value<'a>, InkError> {
        match arena.nodes[idx] {
            Expr::Number(n) => Ok(Value::Integer(n)),
            Expr::Float(f) => Ok(Value::Float(f)),
            Expr::StringLitteral(ref str) => Ok(Value::Text(Cow::Owned(str.to_string()))),
            Expr::Null => Ok(Value::Null),
            Expr::Bool(b) => Ok(Value::Integer(b as u8 as i64)),
            Expr::ColumnRef(col_idx) => match row {
                Some(row) => row.column(col_idx),
                _ => Err(InkError::runtime(
                    "Runtime column references are not supported. Only compile time references are allowed",
                )),
            },

            Expr::Add(l, r) => {
                let lhs = Self::eval(arena, l, row)?;
                lhs.try_add(&Self::eval(arena, r, row)?)
            }
            Expr::Substract(l, r) => {
                let lhs = Self::eval(arena, l, row)?;
                lhs.try_sub(&Self::eval(arena, r, row)?)
            }
            Expr::Multiply(l, r) => {
                let lhs = Self::eval(arena, l, row)?;
                lhs.try_mul(&Self::eval(arena, r, row)?)
            }
            Expr::Devide(l, r) => {
                let lhs = Self::eval(arena, l, row)?;
                lhs.try_div(&Self::eval(arena, r, row)?)
            }
            Expr::Neg(x) => Self::eval(arena, x, row)?.try_mul(&Value::Integer(-1)),

            Expr::Not(expr) => {
                if !Self::eval(arena, expr, row)?.to_bool() {
                    return Ok(Value::Integer(1));
                }
                Ok(Value::Integer(0))
            }

            Expr::And { left, right } => {
                if Self::eval(arena, left, row)?.to_bool()
                    && Self::eval(arena, right, row)?.to_bool()
                {
                    return Ok(Value::Integer(1));
                }
                Ok(Value::Integer(0))
            }

            Expr::Or { left, right } => {
                if Self::eval(arena, left, row)?.to_bool()
                    || Self::eval(arena, right, row)?.to_bool()
                {
                    return Ok(Value::Integer(1));
                }
                Ok(Value::Integer(0))
            }

            Expr::BinaryOp {
                left,
                ref op,
                right,
            } => match op {
                BinaryOperator::Eq => {
                    if Self::eval(arena, left, row)? == Self::eval(arena, right, row)? {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::NotEq => {
                    if Self::eval(arena, left, row)? != Self::eval(arena, right, row)? {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::Gt => {
                    if Self::eval(arena, left, row)? > Self::eval(arena, right, row)? {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::Ge => {
                    if Self::eval(arena, left, row)? >= Self::eval(arena, right, row)? {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::Lt => {
                    if Self::eval(arena, left, row)? < Self::eval(arena, right, row)? {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::Le => {
                    if Self::eval(arena, left, row)? <= Self::eval(arena, right, row)? {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }
            },
            _ => Err(InkError::runtime(format!(
                "Cannot evaluate node: {}",
                render_expr(arena, idx, None)
            ))),
        }
    }
}

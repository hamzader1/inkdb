use std::borrow::Cow;

use crate::errors::SqliteError;
use crate::record::Value;
use crate::sql::ast::{BinaryOperator, Expr};
use crate::sql::parser::ExprArena;

pub struct Eval;
impl Eval {
    pub fn eval_row(arena: &ExprArena, idx: usize, row: &[Value<'static>]) -> Value<'static> {
        match arena.nodes[idx] {
            Expr::Number(n) => Value::Integer(n),
            Expr::Float(f) => Value::Float(f),
            Expr::StringLitteral(ref str) => Value::Text(Cow::Owned(str.to_string())),
            Expr::Bool(b) => Value::Integer(b as u8 as i64),
            Expr::ColumnRef(col_idx) => row[col_idx].into_owned(),
            Expr::Add(l, r) => Self::eval_row(arena, l, row) + Self::eval_row(arena, r, row),
            Expr::Substract(l, r) => Self::eval_row(arena, l, row) - Self::eval_row(arena, r, row),
            Expr::Multiply(l, r) => Self::eval_row(arena, l, row) * Self::eval_row(arena, r, row),
            Expr::Devide(l, r) => Self::eval_row(arena, l, row) / Self::eval_row(arena, r, row),
            Expr::Neg(x) => Value::Integer(-1) * Self::eval_row(arena, x, row), // Limited to '-' for now.
            Expr::Not(expr) => {
                if !Self::eval_row(arena, expr, row).to_bool() {
                    return Value::Integer(1);
                }
                Value::Integer(0)
            }
            Expr::And { left, right } => {
                if Self::eval_row(arena, left, row).to_bool()
                    && Self::eval_row(arena, right, row).to_bool()
                {
                    return Value::Integer(1);
                }
                Value::Integer(0)
            }
            Expr::Or { left, right } => {
                if Self::eval_row(arena, left, row).to_bool()
                    || Self::eval_row(arena, right, row).to_bool()
                {
                    return Value::Integer(1);
                }
                Value::Integer(0)
            }
            Expr::BinaryOp {
                left,
                ref op,
                right,
            } => match op {
                BinaryOperator::Eq => {
                    if Self::eval_row(arena, left, row) == Self::eval_row(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::NotEq => {
                    if Self::eval_row(arena, left, row) != Self::eval_row(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::Gt => {
                    if Self::eval_row(arena, left, row) > Self::eval_row(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::Ge => {
                    if Self::eval_row(arena, left, row) >= Self::eval_row(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::Lt => {
                    if Self::eval_row(arena, left, row) < Self::eval_row(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::Le => {
                    if Self::eval_row(arena, left, row) <= Self::eval_row(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
            },
            _ => todo!(),
        }
    }
    pub fn eval(arena: &ExprArena, idx: usize) -> Result<Value<'static>, SqliteError> {
        match arena.nodes[idx] {
            Expr::Number(n) => Ok(Value::Integer(n)),
            Expr::Float(f) => Ok(Value::Float(f)),
            Expr::StringLitteral(ref str) => Ok(Value::Text(Cow::Owned(str.to_string()))),
            Expr::Bool(b) => Ok(Value::Integer(b as u8 as i64)),
            Expr::ColumnRef(_) => Err(SqliteError::RuntimeError("Columns are not allowed".into())),

            Expr::Add(l, r) => Ok(Self::eval(arena, l)? + Self::eval(arena, r)?),
            Expr::Substract(l, r) => Ok(Self::eval(arena, l)? - Self::eval(arena, r)?),
            Expr::Multiply(l, r) => Ok(Self::eval(arena, l)? * Self::eval(arena, r)?),
            Expr::Devide(l, r) => Ok(Self::eval(arena, l)? / Self::eval(arena, r)?),
            Expr::Neg(x) => Ok(Value::Integer(-1) * Self::eval(arena, x)?),
            Expr::Not(expr) => {
                if !Self::eval(arena, expr)?.to_bool() {
                    return Ok(Value::Integer(1));
                }
                Ok(Value::Integer(0))
            }

            Expr::And { left, right } => {
                if Self::eval(arena, left)?.to_bool() && Self::eval(arena, right)?.to_bool() {
                    Ok(Value::Integer(1))
                } else {
                    Ok(Value::Integer(0))
                }
            }

            Expr::Or { left, right } => {
                if Self::eval(arena, left)?.to_bool() || Self::eval(arena, right)?.to_bool() {
                    Ok(Value::Integer(1))
                } else {
                    Ok(Value::Integer(0))
                }
            }

            Expr::BinaryOp {
                left,
                ref op,
                right,
            } => match op {
                BinaryOperator::Eq => {
                    if Self::eval(arena, left)? == Self::eval(arena, right)? {
                        Ok(Value::Integer(1))
                    } else {
                        Ok(Value::Integer(0))
                    }
                }

                BinaryOperator::NotEq => {
                    if Self::eval(arena, left)? != Self::eval(arena, right)? {
                        Ok(Value::Integer(1))
                    } else {
                        Ok(Value::Integer(0))
                    }
                }

                BinaryOperator::Gt => {
                    if Self::eval(arena, left)? > Self::eval(arena, right)? {
                        Ok(Value::Integer(1))
                    } else {
                        Ok(Value::Integer(0))
                    }
                }

                BinaryOperator::Ge => {
                    if Self::eval(arena, left)? >= Self::eval(arena, right)? {
                        Ok(Value::Integer(1))
                    } else {
                        Ok(Value::Integer(0))
                    }
                }

                BinaryOperator::Lt => {
                    if Self::eval(arena, left)? < Self::eval(arena, right)? {
                        Ok(Value::Integer(1))
                    } else {
                        Ok(Value::Integer(0))
                    }
                }

                BinaryOperator::Le => {
                    if Self::eval(arena, left)? <= Self::eval(arena, right)? {
                        Ok(Value::Integer(1))
                    } else {
                        Ok(Value::Integer(0))
                    }
                }
            },

            _ => todo!(),
        }
    }
}

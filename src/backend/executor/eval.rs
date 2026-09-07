use std::borrow::Cow;

use crate::errors::SqliteError;
use crate::record::Value;
use crate::sql::ast::{BinaryOperator, Expr};
use crate::sql::parser::ExprArena;

pub struct Eval;
impl Eval {
    pub fn eval(
        arena: &ExprArena,
        idx: usize,
        row: Option<&[Value<'static>]>,
    ) -> Result<Value<'static>, SqliteError> {
        match arena.nodes[idx] {
            Expr::Number(n) => Ok(Value::Integer(n)),
            Expr::Float(f) => Ok(Value::Float(f)),
            Expr::StringLitteral(ref str) => {
                Ok(Value::Text(Cow::Owned(str.to_string())))
            }
            Expr::Bool(b) => Ok(Value::Integer(b as u8 as i64)),
            Expr::ColumnRef(col_idx) => match row {
                Some(row) => Ok(row[col_idx].into_owned()),
                _ => Err(SqliteError::Runtime(
                    "Cannot evaluate a column reference without a row: LIMIT and constant expressions must not mention columns".into(),
                )),
            },

            Expr::Add(l, r) => {
                Ok(Self::eval(arena, l, row)? + Self::eval(arena, r, row)?)
            }
            Expr::Substract(l, r) => {
                Ok(Self::eval(arena, l, row)? - Self::eval(arena, r, row)?)
            }
            Expr::Multiply(l, r) => {
                Ok(Self::eval(arena, l, row)? * Self::eval(arena, r, row)?)
            }
            Expr::Devide(l, r) => {
                Ok(Self::eval(arena, l, row)? / Self::eval(arena, r, row)?)
            }
            Expr::Neg(x) => {
                Ok(Value::Integer(-1) * Self::eval(arena, x, row)?)
            }

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
                    if Self::eval(arena, left, row)?
                        == Self::eval(arena, right, row)?
                    {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::NotEq => {
                    if Self::eval(arena, left, row)?
                        != Self::eval(arena, right, row)?
                    {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::Gt => {
                    if Self::eval(arena, left, row)?
                        > Self::eval(arena, right, row)?
                    {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::Ge => {
                    if Self::eval(arena, left, row)?
                        >= Self::eval(arena, right, row)?
                    {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::Lt => {
                    if Self::eval(arena, left, row)?
                        < Self::eval(arena, right, row)?
                    {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }

                BinaryOperator::Le => {
                    if Self::eval(arena, left, row)?
                        <= Self::eval(arena, right, row)?
                    {
                        return Ok(Value::Integer(1));
                    }
                    Ok(Value::Integer(0))
                }
            },

            _ => todo!(),
        }
    }
}

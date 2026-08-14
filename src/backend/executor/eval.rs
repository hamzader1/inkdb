use std::borrow::Cow;

use crate::record::Value;
use crate::sql::ast::{BinaryOperator, Expr};
use crate::sql::parser::ExprArena;

pub struct Eval;
impl Eval {
    pub fn eval(arena: &ExprArena, idx: usize, row: &[Value<'static>]) -> Value<'static> {
        match arena.nodes[idx] {
            Expr::Number(n) => Value::Integer(n),
            Expr::Float(f) => Value::Float(f),
            Expr::StringLitteral(ref str) => Value::Text(Cow::Owned(str.to_string())),
            Expr::Bool(b) => Value::Integer(b as u8 as i64),
            Expr::ColumnRef(col_idx) => row[col_idx].into_owned(),
            Expr::Add(l, r) => Self::eval(arena, l, row) + Self::eval(arena, r, row),
            Expr::Substract(l, r) => Self::eval(arena, l, row) - Self::eval(arena, r, row),
            Expr::Multiply(l, r) => Self::eval(arena, l, row) * Self::eval(arena, r, row),
            Expr::Devide(l, r) => Self::eval(arena, l, row) / Self::eval(arena, r, row),
            Expr::Neg(x) => Value::Integer(-1) * Self::eval(arena, x, row), // Limited to '-' for now.
            Expr::And { left, right } => {
                if Self::eval(arena, left, row).to_bool() && Self::eval(arena, right, row).to_bool()
                {
                    return Value::Integer(1);
                }
                Value::Integer(0)
            }
            Expr::Or { left, right } => {
                if Self::eval(arena, left, row).to_bool() || Self::eval(arena, right, row).to_bool()
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
                    if Self::eval(arena, left, row) == Self::eval(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::NotEq => {
                    if Self::eval(arena, left, row) != Self::eval(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::Gt => {
                    if Self::eval(arena, left, row) > Self::eval(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::Ge => {
                    if Self::eval(arena, left, row) >= Self::eval(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::Lt => {
                    if Self::eval(arena, left, row) < Self::eval(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
                BinaryOperator::Le => {
                    if Self::eval(arena, left, row) <= Self::eval(arena, right, row) {
                        return Value::Integer(1);
                    }
                    Value::Integer(0)
                }
            },
            _ => todo!(),
        }
    }
}

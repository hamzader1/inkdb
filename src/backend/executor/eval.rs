use std::borrow::Cow;

use crate::record::Value;
use crate::sql::ast::Expr;
use crate::sql::parser::Arena;

pub struct Eval;
impl Eval {
    pub fn eval(arena: &Arena, idx: usize, row: &[Value<'static>]) -> Value<'static> {
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
            _ => todo!(),
        }
    }
}

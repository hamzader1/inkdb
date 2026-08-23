use std::borrow::Cow;

use super::ast::{Ast, Column, SelectStmt};
use super::parser::Parser;
use super::tokens::TokenKind::{self, *};
use crate::errors::SqliteError;
use crate::record::Value;
use crate::sql::ast::{Expr, InsertStmt};

impl Parser {
    pub fn parse_select(&mut self) -> Result<Ast, SqliteError> {
        self.expect(Select)?;
        let mut columns = Vec::new();
        loop {
            if self.eat(Star) {
                let idx = self.arena.nodes.len();
                self.arena.nodes.push(Expr::Star);
                columns.push(idx);
            } else {
                columns.push(self.parse_expression()?);
            }
            if !self.eat(Comma) {
                // anything, BUT COMMA
                break;
            }
        }
        self.expect(From)?;
        let table_name = self.expect_ident()?.to_lowercase();
        let mut where_clause: Option<usize> = None;
        if self.eat(Where) {
            where_clause = Some(self.parse_expression()?);
        }
        let mut limit: Option<usize> = None;
        if self.eat(Limit) {
            limit = Some(self.parse_expression()?);
        }

        Ok(Ast::SelectStmtAst(SelectStmt {
            table_name,
            arena: self.arena.clone(),
            columns,
            where_clause,
            limit,
        }))
    }

    pub fn parse_insert(&mut self) -> Result<Ast, SqliteError> {
        self.expect(Insert)?;
        self.expect(Into)?;
        let table_name = self.expect_ident()?.to_ascii_lowercase();
        self.expect(Values)?;
        self.expect(LeftParen)?;
        let mut columns: Vec<std::string::String> = Vec::new();
        let mut values: Vec<crate::record::Value<'static>> = Vec::new();
        while !self.at(RightParen) {
            match self.peek() {
                Some(String(s)) => {
                    let cow: Cow<'_, str> = Cow::Owned(s.to_owned());
                    values.push(Value::Text(cow));
                    self.next_token();
                }
                Some(NumberVar(n)) => {
                    values.push(Value::Integer(*n));
                    self.next_token();
                }
                Some(FloatVar(f)) => {
                    values.push(Value::Float(*f));
                    self.next_token();
                }
                Some(BoolVar(b)) => {
                    values.push(Value::Integer(*b as u8 as _));
                    self.next_token();
                }
                Some(Null) => {
                    values.push(Value::Null);
                    self.next_token();
                }
                _ => panic!("This value is not allowed in insertions values"),
            }
            if !self.eat(Comma) {
                break;
            }
        }
        self.eat(RightParen);

        Ok(Ast::InsertStmtAst(InsertStmt {
            table_name,
            columns,
            values,
        }))
    }
}

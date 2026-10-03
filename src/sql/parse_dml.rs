use std::borrow::Cow;
use std::cmp::Ordering::Equal;

use super::ast::{Ast, OrderBy, SelectStmt, UpdateStmt};
use super::parser::Parser;
use super::tokens::TokenKind::*;
use crate::InkResult;
use crate::errors::InkError;
use crate::record::Value;
use crate::sql::ast::{DeleteStmt, Expr, InsertStmt};

impl Parser {
    pub(crate) fn parse_select(&mut self) -> Result<Ast, InkError> {
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
        let mut orderby: Option<OrderBy> = None;
        if self.eat(Order) {
            self.expect(By);
            let index = self.parse_expression()?;
            let desc = if self.eat(Desc) {
                true
            } else {
                /*
                 * Both are valid.
                 * Either we consume Asc token if its exist, otherwise
                 * asc is the default
                 */
                self.eat(Asc);
                false
            };
            orderby = Some(OrderBy::new(index, desc));
        }
        let mut limit: Option<usize> = None;
        if self.eat(Limit) {
            limit = Some(self.parse_expression()?);
        }

        self.expect_eof()?;
        Ok(Ast::SelectStmtAst(SelectStmt {
            table_name,
            arena: self.arena.take(),
            columns,
            where_clause,
            limit,
            orderby,
        }))
    }

    pub(crate) fn parse_insert(&mut self) -> Result<Ast, InkError> {
        self.expect(Insert)?;
        self.expect(Into)?;
        let table_name = self.expect_ident()?.to_ascii_lowercase();
        let mut columns: Vec<std::string::String> = Vec::new();
        if self.eat(LeftParen) {
            while !self.at(RightParen) {
                columns.push(self.expect_ident()?);
                if !self.at(Comma) {
                    break;
                }
                self.eat(Comma);
            }
            self.expect(RightParen)?;
        }
        self.expect(Values)?;
        let mut values: Vec<_> = Vec::new();
        loop {
            let mut current_values = Vec::new();
            self.expect(LeftParen)?;
            while !self.at(RightParen) {
                current_values.push(self.parse_expression()?);
                if !self.eat(Comma) {
                    break;
                }
            }
            values.push(current_values);
            self.expect(RightParen)?;
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect_eof()?;

        Ok(Ast::InsertStmtAst(InsertStmt {
            table_name,
            columns,
            values,
            arena: self.arena.take(),
        }))
    }
    pub(crate) fn parse_delete(&mut self) -> InkResult<Ast> {
        self.expect(Delete)?;
        self.expect(From)?;
        let table_name = self.expect_ident()?;
        let mut where_clause = None;
        let mut arena = None;
        if self.eat(Where) {
            where_clause = Some(self.parse_expression()?);
            arena = Some(self.arena.take());
        }
        self.expect_eof()?;
        Ok(Ast::DeleteStmtAst(DeleteStmt {
            table_name,
            arena,
            where_clause,
        }))
    }
    pub(crate) fn parse_update(&mut self) -> InkResult<Ast> {
        self.expect(Update)?;
        let table_name = self.expect_ident()?;
        self.expect(Set)?;
        let mut affected_columns = Vec::new();
        loop {
            let col_name = self.parse_factor()?;
            self.expect(Equals)?;
            let expr = self.parse_expression()?;
            affected_columns.push((col_name, expr));
            if !self.eat(Comma) {
                break;
            }
        }
        let mut where_clause = None;
        if self.eat(Where) {
            let predicate = self.parse_expression()?;
            where_clause = Some(predicate);
        }
        self.expect_eof()?;
        Ok(Ast::UpdateStmtAst(UpdateStmt::new(
            table_name,
            affected_columns,
            where_clause,
            self.arena.take(),
        )))
    }
}

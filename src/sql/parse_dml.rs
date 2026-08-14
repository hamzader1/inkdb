use super::ast::{Ast, Column, SelectStmt};
use super::parser::Parser;
use super::tokens::TokenKind::{self, *};
use crate::errors::SqliteError;
use crate::sql::ast::Expr;

impl Parser {
    pub fn parse_select(&mut self) -> Result<Ast, SqliteError> {
        self.expect(Select);
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
        let table_name = self.expect_ident()?;
        let mut where_clause: Option<usize> = None;
        if self.eat(Where) {
            where_clause = Some(self.parse_expression()?);
        }

        Ok(Ast::SelectStmtAst(SelectStmt {
            table_name,
            arena: self.arena.clone(),
            columns,
            where_clause,
        }))
    }
}

use super::ast::{Ast, Column, SelectStmt};
use super::parser::Parser;
use super::tokens::TokenKind::{self, *};
use crate::errors::SqliteError;

impl Parser {
    pub fn parse_select(&mut self) -> Result<Ast, SqliteError> {
        self.expect(Select);
        let mut columns = Vec::new();
        loop {
            columns.push(self.parse_expression()?);
            if !self.eat(Comma) {
                // anything, BUT COMMA
                break;
            }
        }
        self.expect(From)?;
        let table_name = self.expect_ident()?;

        // limited to no `where` so far
        Ok(Ast::SelectStmtAst(SelectStmt {
            table_name,
            arena: self.arena.clone(),
            columns,
            where_clause: None,
        }))
    }
}

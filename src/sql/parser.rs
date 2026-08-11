use super::ast::Ast;
use crate::errors::SqliteError::{self, *};

use super::tokens::{
    Token,
    TokenKind::{self, *},
};
use std::string::String;

pub struct Parser {
    pub tokens: Vec<Token>,
    pub pos: usize,
}

impl Parser {
    pub fn parse(tokens: Vec<Token>) -> Result<Ast, SqliteError> {
        let mut parser = Parser { tokens, pos: 0 };
        parser.parse_statement()
    }
    pub fn at(&self, t_kind: &TokenKind) -> bool {
        if let Some(t) = self.tokens.get(self.pos)
            && &t.kind == t_kind
        {
            return true;
        }
        false
    }

    pub fn eat(&mut self, t_kind: &TokenKind) -> bool {
        if self.at(t_kind) {
            self.pos += 1;
            return true;
        }
        false
    }

    pub fn peek(&self) -> Option<&TokenKind> {
        if let Some(t) = self.tokens.get(self.pos) {
            return Some(&t.kind);
        }
        None
    }

    pub fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    pub fn expect(&mut self, t_kind: &TokenKind) -> Result<(), SqliteError> {
        if !self.at(t_kind) {
            return Err(RuntimeError(
                "Given token does not match the current token".into(),
            ));
        }
        self.pos += 1;
        Ok(())
    }
    pub fn expect_ident(&mut self) -> Result<String, SqliteError> {
        match self.next() {
            Some(t) => match t.kind {
                Identifier(x) => Ok(x),
                any => Err(RuntimeError("Expected identifier".into())),
            },
            None => Err(RuntimeError(
                "Unexpected end of input, Expected identifier".into(),
            )),
        }
    }
    pub fn parse_statement(&mut self) -> Result<Ast, SqliteError> {
        match self.peek() {
            Some(Create) => self.parse_create(),
            _ => todo!(),
        }
    }
}

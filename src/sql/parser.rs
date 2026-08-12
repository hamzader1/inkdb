use super::ast::Ast;
use crate::errors::SqliteError::{self, *};

use super::tokens::{
    Token,
    TokenKind::{self, *},
};
use std::string::String;

use super::ast::Expr;
#[derive(Debug, Default, Clone)]
pub struct Arena {
    pub nodes: Vec<Expr>,
}
impl Arena {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }
    pub fn push(&mut self, expr: Expr) -> usize {
        self.nodes.push(expr);
        self.nodes.len() - 1
    }
}
pub struct Parser {
    pub tokens: Vec<Token>,
    pub pos: usize,
    pub arena: Arena,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            arena: Arena::new(),
        }
    }
    pub fn parse(tokens: Vec<Token>) -> Result<Ast, SqliteError> {
        let mut parser = Parser {
            tokens,
            pos: 0,
            arena: Arena::new(),
        };
        parser.parse_statement()
    }
    pub fn at(&self, t_kind: TokenKind) -> bool {
        if let Some(t) = self.tokens.get(self.pos)
            && t_kind == t.kind
        {
            return true;
        }
        false
    }

    pub fn eat(&mut self, t_kind: TokenKind) -> bool {
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

    pub fn expect(&mut self, t_kind: TokenKind) -> Result<(), SqliteError> {
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

    // pub fn expect_string(&mut self) -> Result<String, SqliteError> {
    //     match self.next() {
    //         Some(t) => match t.kind {
    //             String(x) => Ok(x),
    //             any => Err(RuntimeError("Expected identifier".into())),
    //         },
    //         None => Err(RuntimeError(
    //             "Unexpected end of input, Expected identifier".into(),
    //         )),
    //     }
    // }
    // pub fn expect_int(&mut self) -> Result<i64, SqliteError> {
    //     match self.next() {
    //         Some(t) => match t.kind {
    //             NumberVar(x) => Ok(x),
    //             any => Err(RuntimeError("Expected identifier".into())),
    //         },
    //         None => Err(RuntimeError(
    //             "Unexpected end of input, Expected identifier".into(),
    //         )),
    //     }
    // }
    // pub fn at_ident(&self) -> bool {
    //     if let Some(tkind) = self.peek() {
    //         return matches!(tkind, Identifier(_));
    //     }
    //     false
    // }
    // pub fn at_string(&self) -> bool {
    //     if let Some(tkind) = self.peek() {
    //         return matches!(tkind, String(_));
    //     }
    //     false
    // }

    // pub fn at_number(&self) -> bool {
    //     if let Some(tkind) = self.peek() {
    //         return matches!(tkind, NumberVar(_));
    //     }
    //     false
    // }
    // pub fn at_bool(&self) -> bool {
    //     if let Some(tkind) = self.peek() {
    //         return matches!(tkind, BoolVar(_));
    //     }
    //     false
    // }
    pub fn parse_statement(&mut self) -> Result<Ast, SqliteError> {
        match self.peek() {
            Some(Create) => self.parse_create(),
            Some(Select) => self.parse_select(),
            _ => todo!(),
        }
    }
}

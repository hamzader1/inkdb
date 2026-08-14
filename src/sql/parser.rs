use super::{ast::Ast, tokens::Span};
use crate::errors::SqliteError::{self, *};

use super::tokens::{
    Token,
    TokenKind::{self, *},
};
use std::{rc::Rc, string::String};

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
    pub query: Rc<str>,
    pub tokens: Vec<Token>,
    pub pos: usize,
    pub arena: Arena,
}

impl Parser {
    pub fn new(query: Rc<str>, tokens: Vec<Token>) -> Self {
        Self {
            query,
            tokens,
            pos: 0,
            arena: Arena::new(),
        }
    }
    pub fn parse(query: Rc<str>, tokens: Vec<Token>) -> Result<Ast, SqliteError> {
        let mut parser = Parser {
            query,
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

    pub fn next_token(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    // Should be called only if we know there
    // is at least one token remaining
    pub fn current_token_span(&self) -> Span {
        self.tokens.get(self.pos).unwrap().span.clone()
    }

    pub fn expect(&mut self, t_kind: TokenKind) -> Result<(), SqliteError> {
        if !self.at(t_kind.clone()) {
            match self.peek() {
                Some(t) => {
                    return Err(TypeMismatch {
                        input: Rc::clone(&self.query),
                        expected_token: t_kind,
                        actual: t.clone(),
                        span: self.current_token_span(),
                    });
                }
                _ => {
                    return Err(UnexpectedEndOfExpression {
                        input: Rc::clone(&self.query),
                        tkind: t_kind,
                        span: self.default_end_span(),
                    });
                }
            }
        }
        self.pos += 1;
        Ok(())
    }
    pub fn default_end_span(&self) -> Span {
        let s = Span(self.query.len(), self.query.len() + 1);
        dbg!(&s);
        s
    }
    pub fn expect_ident(&mut self) -> Result<String, SqliteError> {
        match self.peek() {
            Some(TokenKind::Identifier(_)) => match self.next_token() {
                Some(Token {
                    kind: TokenKind::Identifier(x),
                    ..
                }) => Ok(x),

                _ => unreachable!(),
            },

            Some(tkind) => Err(ExpectedIdentifier {
                input: Rc::clone(&self.query),
                tkind: tkind.clone(),
                span: self.current_token_span(),
            }),

            None => Err(UnexpectedEndOfExpression {
                input: Rc::clone(&self.query),
                tkind: TokenKind::Identifier(String::new()),
                span: self.default_end_span(),
            }),
        }
    }

    pub fn parse_statement(&mut self) -> Result<Ast, SqliteError> {
        match self.peek() {
            Some(Create) => self.parse_create(),
            Some(Select) => self.parse_select(),
            _ => todo!(),
        }
    }
}

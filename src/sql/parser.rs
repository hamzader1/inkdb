use super::ast::Ast;
use super::tokens::{
    Span, Token,
    TokenKind::{self, *},
};
use std::string::String;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn parse(tokens: Vec<Token>) -> Ast {
        let mut parser = Parser { tokens, pos: 0 };
        Ast::Null
    }
    fn current(&self) -> Option<&TokenKind> {
        if let Some(t) = self.tokens.get(self.pos) {
            return Some(&t.kind);
        }
        None
    }

    fn peek_ahead_by(&self, n: usize) -> Option<&TokenKind> {
        if let Some(t) = self.tokens.get(self.pos + n) {
            return Some(&t.kind);
        }
        None
    }

    fn peek(&self) -> Option<&TokenKind> {
        if let Some(t) = self.tokens.get(self.pos) {
            return Some(&t.kind);
        }
        None
    }
    fn advance(&mut self) -> Option<&TokenKind> {
        if let Some(t) = self.tokens.get(self.pos) {
            return Some(&t.kind);
        }
        None
    }
    fn span(&self) -> Option<&Span> {
        if let Some(t) = self.tokens.get(self.pos) {
            return Some(&t.span);
        }
        None
    }

    fn parse_statment(&mut self) -> Ast {
        match self.peek() {
            Some(Create) => {}
            _ => unreachable!(),
        }
        Ast::Null
    }
}

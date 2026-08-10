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
    pub fn parse(tokens: Vec<Token>) -> Ast {
        let mut parser = Parser { tokens, pos: 0 };
        Ast::Null
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

    pub fn expect(&self, t_kind: &TokenKind) {
        if !self.at(t_kind) {
            panic!("Token Mismatch") // temporary panic for now
        }
    }
    pub fn expect_ident(&mut self) -> String {
        match self.next() {
            Some(t) => match t.kind {
                String(x) => x,
                _ => panic!(),
            },
            None => panic!(),
        }
    }

    // pub fn peek_ahead_by(&self, n: usize) -> Option<&TokenKind> {
    //     if let Some(t) = self.tokens.get(self.pos + n) {
    //         return Some(&t.kind);
    //     }
    //     None
    // }

    // pub fn advance(&mut self) -> Option<&TokenKind> {
    //     if let Some(t) = self.tokens.get(self.pos) {
    //         return Some(&t.kind);
    //     }
    //     None
    // }

    // pub fn verify_and_advance(&mut self, t_kind: TokenKind) -> Option<&TokenKind> {
    //     if let Some(t) = self.tokens.get(self.pos) && t_kind == t.kind {
    //         self.pos+=1;
    //         return Some(&t.kind)
    //     }
    //     None
    // }
    // pub fn span(&self) -> Option<&Span> {
    //     if let Some(t) = self.tokens.get(self.pos) {
    //         return Some(&t.span);
    //     }
    //     None
    // }

    // pub fn parse_statment(&mut self) -> Ast {
    //     // match self.advance() {
    //         Some(Create) => {}
    //         _ => unreachable!(),
    //     }
    //     Ast::Null
    // }
}

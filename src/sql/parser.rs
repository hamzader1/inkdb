use super::{
    ast::{
        Ast::{self, ExplainStmtAst},
        ExplainStmt,
    },
    tokens::Span,
};
use crate::errors::{SqliteError, SyntaxErrorKind};

use super::tokens::{
    Token,
    TokenKind::{self, *},
};
use std::{rc::Rc, string::String};

use super::ast::Expr;
#[derive(Debug, Default, Clone)]
pub struct ExprArena {
    pub nodes: Vec<Expr>,
}
impl ExprArena {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }
    pub fn push(&mut self, expr: Expr) -> usize {
        self.nodes.push(expr);
        self.nodes.len() - 1
    }
    pub fn take(&mut self) -> ExprArena {
        Self {
            nodes: std::mem::take(&mut self.nodes),
        }
    }
}
pub struct Parser {
    pub query: Rc<str>,
    pub tokens: Vec<Token>,
    pub pos: usize,
    pub arena: ExprArena,
}

impl Parser {
    pub fn new(query: Rc<str>, tokens: Vec<Token>) -> Self {
        Self {
            query,
            tokens,
            pos: 0,
            arena: ExprArena::new(),
        }
    }
    pub fn parse(query: Rc<str>, tokens: Vec<Token>) -> Result<Ast, SqliteError> {
        let mut parser = Parser {
            query,
            tokens,
            pos: 0,
            arena: ExprArena::new(),
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
    // is at least one token left
    pub fn current_token_span(&self) -> Span {
        self.tokens.get(self.pos).unwrap().span.clone()
    }

    pub fn expect(&mut self, t_kind: TokenKind) -> Result<(), SqliteError> {
        if !self.at(t_kind.clone()) {
            match self.peek() {
                Some(t) => {
                    return Err(SqliteError::syntax(
                        SyntaxErrorKind::TokenMismatch {
                            expected: t_kind,
                            actual: t.clone(),
                        },
                        self.current_token_span(),
                    ));
                }
                _ => {
                    return Err(SqliteError::syntax(
                        SyntaxErrorKind::UnexpectedEndOfExpression(t_kind),
                        self.default_end_span(),
                    ));
                }
            }
        }
        self.pos += 1;
        Ok(())
    }
    pub fn default_end_span(&self) -> Span {
        Span(self.query.len(), self.query.len() + 1)
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

            Some(tkind) => Err(SqliteError::syntax(
                SyntaxErrorKind::ExpectedIdentifier(tkind.clone()),
                self.current_token_span(),
            )),

            None => Err(SqliteError::syntax(
                SyntaxErrorKind::UnexpectedEndOfExpression(TokenKind::Identifier(String::new())),
                self.default_end_span(),
            )),
        }
    }

    pub fn parse_statement(&mut self) -> Result<Ast, SqliteError> {
        match self.peek() {
            Some(Create) => self.parse_create(),
            Some(Explain) => {
                self.expect(Explain)?;
                let query = self.parse_statement()?;
                Ok(ExplainStmtAst(ExplainStmt{
                    query: Box::new(query)
                }))
            },
            Some(Select) => self.parse_select(),
            Some(Insert) => self.parse_insert(),
            Some(Delete) => self.parse_delete(),
            Some(Begin) => {
                self.eat(Begin);
                Ok(Ast::BeginTransaction)
            },
            Some(Commit) => {
                self.eat(Commit);
                Ok(Ast::CommitTransaction)
            }
            Some(RollBack) => {
                self.eat(RollBack);
                Ok(Ast::RollbackTransaction)
            }
            _ => Err(SqliteError::Unsupported(
                "this statement type is not supported yet (only SELECT, INSERT, DELETE, CREATE TABLE and BEGIN/COMMIT/ROLLBACK)".into(),
            )),
        }
    }
}

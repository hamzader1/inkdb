use super::{
    ast::{
        Ast::{self, ExplainStmtAst},
        ExplainStmt,
    },
    tokens::Span,
};
use crate::{
    InkResult,
    errors::{InkError, SyntaxErrorKind},
};

use super::tokens::{
    Token,
    TokenKind::{self, *},
};
use std::{rc::Rc, string::String};

use super::ast::Expr;
/// Expressions for one statement, stored in a flat list.
///
/// Expressions refer to each other by their position in this list rather than
/// by holding their children, so `WHERE 9 + 1 = 10` becomes three entries and
/// a couple of indices.
/// For example [Int(9), Int(1), Add(0, 1)]. The `Add` expression will
/// look at values at index 0 (the number 9) and index 1 (the number 1).
/// Keeping them flat means the parts can be compared and
/// rewritten without walking a tree, and the whole arena is cheap to hand
/// around by reference.
#[derive(Debug, Default, Clone)]
pub struct ExprArena {
    pub(crate) nodes: Vec<Expr>,
}
impl ExprArena {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }
    pub(crate) fn push(&mut self, expr: Expr) -> usize {
        self.nodes.push(expr);
        self.nodes.len() - 1
    }
    /// Move the expressions out and leave the arena empty.
    /// Used when a builder hands its work to the next stage.
    pub(crate) fn take(&mut self) -> ExprArena {
        Self {
            nodes: std::mem::take(&mut self.nodes),
        }
    }
}
/// Turns a list of tokens into a statement.
///
/// This is a recursive descent parser, a type of top down parser that starts
/// at the highest level of the parse tree and works its way down using the
/// production rules of the grammar.
///
/// For more information, see the [`Recursive descent parser`](https://en.wikipedia.org/wiki/Recursive_descent_parser).
///
/// The parser walks through the tokens from left to right, reading one at a
/// time. Expressions are stored in `arena`, and the returned statement refers
/// to them by index. The original query text is also preserved so errors can
/// include it in their messages.
///
/// The grammar is implemented as a chain of small functions, each handling
/// a level of precedence and calling the next level down.
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
    /// Parse one statement from its tokens.
    pub fn parse(query: Rc<str>, tokens: Vec<Token>) -> Result<Ast, InkError> {
        let mut parser = Parser {
            query,
            tokens,
            pos: 0,
            arena: ExprArena::new(),
        };
        parser.parse_statement()
    }
    /// Whether the token we are looking at is this one.
    pub(crate) fn at(&self, t_kind: TokenKind) -> bool {
        if let Some(t) = self.tokens.get(self.pos)
            && t_kind == t.kind
        {
            return true;
        }
        false
    }

    /// Step past the token if it is this one, and say whether we did. Used for
    /// the optional parts of the grammar, where not matching is not an error.
    pub(crate) fn eat(&mut self, t_kind: TokenKind) -> bool {
        if self.at(t_kind) {
            self.pos += 1;
            return true;
        }
        false
    }

    /// The token we are looking at, without stepping past it.
    pub(crate) fn peek(&self) -> Option<&TokenKind> {
        if let Some(t) = self.tokens.get(self.pos) {
            return Some(&t.kind);
        }
        None
    }

    /// Take the token we are looking at and move on. Returns nothing at the end
    /// of the statement.
    pub(crate) fn next_token(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    /// The span of the token we are looking at.
    ///
    /// Only call this when you already know a token is there, because a
    /// statement that ran out is reported through [`Parser::default_end_span`].
    pub(crate) fn current_token_span(&self) -> Span {
        self.tokens.get(self.pos).unwrap().span.clone()
    }

    /// Demand the token we are looking at, and step past it.
    pub(crate) fn expect(&mut self, t_kind: TokenKind) -> Result<(), InkError> {
        if !self.at(t_kind.clone()) {
            match self.peek() {
                Some(t) => {
                    return Err(InkError::syntax(
                        SyntaxErrorKind::TokenMismatch {
                            expected: t_kind,
                            actual: t.clone(),
                        },
                        self.current_token_span(),
                    ));
                }
                _ => {
                    return Err(InkError::syntax(
                        SyntaxErrorKind::UnexpectedEndOfExpression(t_kind),
                        self.default_end_span(),
                    ));
                }
            }
        }
        self.pos += 1;
        Ok(())
    }
    /// Check that nothing is left.
    /// So trailing junk after a complete
    /// statement is caught rather than silently ignored.
    pub(crate) fn expect_eof(&self) -> InkResult<()> {
        if self.peek().is_some() {
            return Err(InkError::syntax(
                SyntaxErrorKind::ExpectedEoi(self.tokens[self.pos].kind.clone()),
                self.current_token_span(),
            ));
        }
        Ok(())
    }
    /// A span just past the end of the statement, used to point at the place a
    /// statement stopped when there was nothing left to point at.
    pub(crate) fn default_end_span(&self) -> Span {
        Span(self.query.len(), self.query.len() + 1)
    }
    /// Read a name, such as a table, column or index name.
    pub(crate) fn expect_ident(&mut self) -> Result<String, InkError> {
        match self.peek() {
            Some(TokenKind::Identifier(_)) => match self.next_token() {
                Some(Token {
                    kind: TokenKind::Identifier(x),
                    ..
                }) => Ok(x),

                _ => unreachable!(),
            },

            Some(tkind) => Err(InkError::syntax(
                SyntaxErrorKind::ExpectedIdentifier(tkind.clone()),
                self.current_token_span(),
            )),

            None => Err(InkError::syntax(
                SyntaxErrorKind::UnexpectedEndOfExpression(TokenKind::Identifier(String::new())),
                self.default_end_span(),
            )),
        }
    }

    /// Read one statement, whichever kind it turns out to be, by looking at the
    /// first token.
    pub(crate) fn parse_statement(&mut self) -> Result<Ast, InkError> {
        match self.peek() {
            Some(Create) => self.parse_create(),
            Some(Explain) => {
                self.expect(Explain)?;
                let query = self.parse_statement()?;
                Ok(ExplainStmtAst(ExplainStmt {
                    query: Box::new(query),
                }))
            }
            Some(Select) => self.parse_select(),
            Some(Drop) => self.parse_drop(),
            Some(Update) => self.parse_update(),
            Some(Insert) => self.parse_insert(),
            Some(Delete) => self.parse_delete(),
            Some(Begin) => {
                self.eat(Begin);
                Ok(Ast::BeginTransaction)
            }
            Some(Commit) => {
                self.eat(Commit);
                Ok(Ast::CommitTransaction)
            }
            Some(RollBack) => {
                self.eat(RollBack);
                Ok(Ast::RollbackTransaction)
            }
            _ => Err(InkError::Unsupported(
                "this statement type is not supported yet ".into(),
            )),
        }
    }
}

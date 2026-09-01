use crate::errors::SqliteError;

use super::tokens::{Span, Token, TokenKind};
use std::rc::Rc;

#[derive(Debug)]
pub struct Lexer<'a> {
    input: &'a str,
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn tokenize(input: &str) -> Result<Vec<Token>, SqliteError> {
        let mut lexer_config = Lexer {
            input,
            chars: input.chars().peekable(),
            pos: 0,
        };

        lexer_config.tokenize_input()
    }
    fn next_char(&mut self) -> Option<char> {
        let ch = self.chars.next()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn tokenize_input(&mut self) -> Result<Vec<Token>, SqliteError> {
        let mut tokens: Vec<Token> = Vec::new();
        let mut parenth_stack: Vec<usize> = Vec::new();
        let mut quotes_stack: Vec<usize> = Vec::new();

        while let Some(&char) = self.chars.peek() {
            match char {
                // Whitespace
                ' ' | '\t' | '\n' | '\r' => {
                    self.next_char();
                }

                // Strings
                '\'' | '"' => {
                    let start = self.pos;
                    quotes_stack.push(start);

                    let quote = char;
                    self.next_char();

                    let mut string = String::new();

                    while let Some(&ch) = self.chars.peek() {
                        if ch == quote {
                            quotes_stack.pop();
                            self.next_char();
                            break;
                        }

                        string.push(ch);
                        self.next_char();
                    }

                    if !quotes_stack.is_empty() {
                        return Err(SqliteError::UnterminatedString {
                            input: self.input.to_string(),
                            position: quotes_stack.pop().unwrap(),
                        });
                    }

                    push_token(&mut tokens, TokenKind::String(string), start, self.pos);
                }

                // Number
                '0'..='9' => {
                    let start = self.pos;

                    let tok = self.extract_number()?;
                    push_token(&mut tokens, tok, start, self.pos);
                }

                // Semicolon
                ';' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Semicolon, start, self.pos);
                }

                // Comma
                ',' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Comma, start, self.pos);
                }

                // Left parenthesis
                '(' => {
                    let start = self.pos;
                    parenth_stack.push(start);
                    self.next_char();

                    push_token(&mut tokens, TokenKind::LeftParen, start, self.pos);
                }

                // Right parenthesis
                ')' => {
                    let start = self.pos;

                    if parenth_stack.is_empty() {
                        return Err(SqliteError::UnmatchedClosingParenthesis {
                            input: self.input.to_string(),
                            position: start,
                        });
                    }

                    parenth_stack.pop();
                    self.next_char();

                    push_token(&mut tokens, TokenKind::RightParen, start, self.pos);
                }

                // =
                '=' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Equals, start, self.pos);
                }

                // !=
                '!' => {
                    let start = self.pos;
                    self.next_char();

                    if let Some('=') = self.chars.peek() {
                        self.next_char();

                        push_token(&mut tokens, TokenKind::NotEquals, start, self.pos);
                    } else {
                        return Err(SqliteError::UnexpectedChar {
                            input: self.input.to_string(),
                            character: char,
                            position: start,
                        });
                    }
                }

                // >
                // >=
                // >>
                '>' => {
                    let start = self.pos;
                    self.next_char();

                    if let Some('=') = self.chars.peek() {
                        self.next_char();

                        push_token(&mut tokens, TokenKind::Ge, start, self.pos);
                    } else if let Some('>') = self.chars.peek() {
                        self.next_char();

                        push_token(&mut tokens, TokenKind::ShiftRight, start, self.pos);
                    } else {
                        push_token(&mut tokens, TokenKind::Gt, start, self.pos);
                    }
                }

                // <
                // <=
                // <<
                '<' => {
                    let start = self.pos;
                    self.next_char();

                    if let Some('=') = self.chars.peek() {
                        self.next_char();

                        push_token(&mut tokens, TokenKind::Le, start, self.pos);
                    } else if let Some('<') = self.chars.peek() {
                        self.next_char();

                        push_token(&mut tokens, TokenKind::ShiftLeft, start, self.pos);
                    } else {
                        push_token(&mut tokens, TokenKind::Lt, start, self.pos);
                    }
                }

                '*' => {
                    let start = self.pos;
                    self.next_char();
                    push_token(&mut tokens, TokenKind::Star, start, self.pos);
                }
                // +
                '+' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Plus, start, self.pos);
                }

                // -
                '-' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Minus, start, self.pos);
                }

                // /
                '/' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Slash, start, self.pos);
                }

                // %
                '%' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Modulus, start, self.pos);
                }

                // ||
                '|' if self.chars.clone().nth(1) == Some('|') => {
                    let start = self.pos;
                    self.next_char();
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Concat, start, self.pos);
                }

                // |
                '|' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::BitOr, start, self.pos);
                }

                // &
                '&' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::BitAnd, start, self.pos);
                }

                // ~
                '~' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Tilde, start, self.pos);
                }

                // .
                '.' => {
                    let start = self.pos;
                    self.next_char();

                    push_token(&mut tokens, TokenKind::Dot, start, self.pos);
                }

                // SQL identifiers / keywords
                _ if char.is_alphabetic() || char == '_' => {
                    let start = self.pos;
                    let mut word = String::new();

                    while let Some(&ch) = self.chars.peek() {
                        if ch.is_alphanumeric() || ch == '_' {
                            word.push(ch);
                            self.next_char();
                        } else {
                            break;
                        }
                    }

                    let upper = word.to_uppercase();

                    let kind = match upper.as_str() {
                        // DDL
                        "CREATE" => TokenKind::Create,
                        "TABLE" => TokenKind::Table,
                        "INDEX" => TokenKind::Index,
                        "DROP" => TokenKind::Drop,
                        "IF" => TokenKind::If,
                        "CONSTRAINT" => TokenKind::Constraint,
                        "PRIMARY" => TokenKind::Primary,
                        "KEY" => TokenKind::Key,
                        "UNIQUE" => TokenKind::Unique,
                        "CHECK" => TokenKind::Check,
                        "DEFAULT" => TokenKind::Default,
                        "WITHOUT" => TokenKind::Without,
                        "ROWID" => TokenKind::Rowid,
                        "ON" => TokenKind::On,
                        "DELETE" => TokenKind::Delete,
                        "UPDATE" => TokenKind::Update,
                        "DESC" => TokenKind::Desc,
                        "ASC" => TokenKind::Asc,

                        // DML
                        "INSERT" => TokenKind::Insert,
                        "INTO" => TokenKind::Into,
                        "VALUES" => TokenKind::Values,
                        "SELECT" => TokenKind::Select,
                        "FROM" => TokenKind::From,
                        "WHERE" => TokenKind::Where,
                        "AND" => TokenKind::And,
                        "OR" => TokenKind::Or,
                        "GROUP" => TokenKind::Group,
                        "BY" => TokenKind::By,
                        "HAVING" => TokenKind::Having,
                        "ORDER" => TokenKind::Order,
                        "LIMIT" => TokenKind::Limit,
                        "DISTINCT" => TokenKind::Distinct,
                        "UNION" => TokenKind::Union,
                        "ALL" => TokenKind::All,
                        "JOIN" => TokenKind::Join,
                        "INNER" => TokenKind::Inner,
                        "OUTER" => TokenKind::Outer,
                        "LEFT" => TokenKind::Left,
                        "RIGHT" => TokenKind::Right,
                        "FULL" => TokenKind::Full,
                        "CROSS" => TokenKind::Cross,
                        "AS" => TokenKind::As,
                        "ROLLBACK" => TokenKind::RollBack,

                        // Expressions
                        "IN" => TokenKind::In,
                        "BETWEEN" => TokenKind::Between,
                        "LIKE" => TokenKind::Like,
                        "IS" => TokenKind::Is,
                        "NOT" => TokenKind::Not,
                        "EXISTS" => TokenKind::Exists,
                        "CASE" => TokenKind::Case,
                        "WHEN" => TokenKind::When,
                        "THEN" => TokenKind::Then,
                        "ELSE" => TokenKind::Else,
                        "END" => TokenKind::End,
                        "CAST" => TokenKind::Cast,
                        "BEGIN" => TokenKind::Begin,
                        "COMMIT" => TokenKind::Commit,

                        // NULL-related keywords
                        "ISNULL" => TokenKind::IsNull,
                        "NOTNULL" => TokenKind::NotNull,

                        // NULL
                        "NULL" => TokenKind::Null,

                        // Boolean literals
                        "TRUE" => TokenKind::BoolVar(true),
                        "FALSE" => TokenKind::BoolVar(false),

                        "BOOL" => TokenKind::Bool,
                        "INT" | "INTEGER" => TokenKind::Integer,
                        "TEXT" => TokenKind::Text,
                        "FLOAT" | "DOUBLE" | "REAL" => TokenKind::Float,
                        "BLOB" => TokenKind::Blob,

                        // Anything else is an identifier
                        _ => TokenKind::Identifier(word),
                    };

                    push_token(&mut tokens, kind, start, self.pos);
                }

                // Anything unsupported
                _ => {
                    return Err(SqliteError::UnexpectedChar {
                        input: self.input.to_string(),
                        character: char,
                        position: self.pos,
                    });
                }
            }
        }

        // Check for unclosed '('
        if let Some(start) = parenth_stack.pop() {
            return Err(SqliteError::UnterminatedParenthsis {
                input: self.input.to_string(),
                position: start,
            });
        }

        Ok(tokens)
    }
    fn extract_number(&mut self) -> Result<TokenKind, SqliteError> {
        let start = self.pos;
        let mut number = String::new();

        // Integer part
        while let Some(&ch) = self.chars.peek() {
            if ch.is_ascii_digit() {
                number.push(ch);
                self.next_char();
            } else {
                break;
            }
        }

        // Decimal part
        if let Some('.') = self.chars.peek() {
            number.push('.');
            self.next_char();

            let decimal_start = self.pos;

            while let Some(&ch) = self.chars.peek() {
                if ch.is_ascii_digit() {
                    number.push(ch);
                    self.next_char();
                } else {
                    break;
                }
            }

            if self.pos == decimal_start {
                return Err(SqliteError::InvalidNumber {
                    input: self.input.to_string(),
                    start,
                    end: self.pos,
                });
            }
            return Ok(TokenKind::FloatVar(number.parse().unwrap()));
        }

        // TODO HANDLE ERRORS
        Ok(TokenKind::NumberVar(number.parse().unwrap()))
    }
}

pub fn push_token(collector: &mut Vec<Token>, tk_kind: TokenKind, start: usize, end: usize) {
    collector.push(Token {
        kind: tk_kind,
        span: Span(start, end),
    });
}

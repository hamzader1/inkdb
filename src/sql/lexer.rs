use crate::errors::{InkError, SyntaxErrorKind};

use super::tokens::{Span, Token, TokenKind};

#[derive(Debug)]
pub struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn tokenize(input: &'a str) -> Result<Vec<Token>, InkError> {
        let mut lexer = Lexer {
            chars: input.chars().peekable(),
            pos: 0,
        };
        lexer.tokenize_input()
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn next_char(&mut self) -> Option<char> {
        let ch = self.chars.next()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn emit(&self, tokens: &mut Vec<Token>, kind: TokenKind, start: usize) {
        tokens.push(Token {
            kind,
            span: Span(start, self.pos),
        });
    }

    fn tokenize_input(&mut self) -> Result<Vec<Token>, InkError> {
        let mut tokens: Vec<Token> = Vec::new();
        let mut depth: usize = 0;
        let mut first_open: Option<usize> = None;

        while let Some(ch) = self.peek() {
            match ch {
                c if c.is_whitespace() => {
                    self.next_char();
                }
                ';' => {
                    self.next_char();
                }
                '\'' | '"' => self.lex_string(&mut tokens)?,
                c if c.is_ascii_digit() => self.lex_number(&mut tokens)?,
                ',' => {
                    let start = self.pos;
                    self.next_char();
                    self.emit(&mut tokens, TokenKind::Comma, start);
                }
                '(' => {
                    let start = self.pos;
                    if first_open.is_none() {
                        first_open = Some(start);
                    }
                    depth += 1;
                    self.next_char();
                    self.emit(&mut tokens, TokenKind::LeftParen, start);
                }
                ')' => {
                    let start = self.pos;
                    if depth == 0 {
                        return Err(InkError::syntax(
                            SyntaxErrorKind::UnmatchedClosingParenthesis,
                            Span(start, start + 1),
                        ));
                    }
                    depth -= 1;
                    if depth == 0 {
                        first_open = None;
                    }
                    self.next_char();
                    self.emit(&mut tokens, TokenKind::RightParen, start);
                }
                '=' => {
                    let start = self.pos;
                    self.next_char();
                    self.emit(&mut tokens, TokenKind::Equals, start);
                }
                '!' => {
                    let start = self.pos;
                    self.next_char();
                    if self.peek() == Some('=') {
                        self.next_char();
                        self.emit(&mut tokens, TokenKind::NotEquals, start);
                    } else {
                        return Err(InkError::syntax(
                            SyntaxErrorKind::UnexpectedChar(ch),
                            Span(start, self.pos),
                        ));
                    }
                }
                '>' => {
                    let start = self.pos;
                    self.next_char();
                    if self.peek() == Some('=') {
                        self.next_char();
                        self.emit(&mut tokens, TokenKind::Ge, start);
                    } else {
                        self.emit(&mut tokens, TokenKind::Gt, start);
                    }
                }
                '<' => {
                    let start = self.pos;
                    self.next_char();
                    if self.peek() == Some('=') {
                        self.next_char();
                        self.emit(&mut tokens, TokenKind::Le, start);
                    } else {
                        self.emit(&mut tokens, TokenKind::Lt, start);
                    }
                }
                '*' => {
                    let start = self.pos;
                    self.next_char();
                    self.emit(&mut tokens, TokenKind::Star, start);
                }
                '+' => {
                    let start = self.pos;
                    self.next_char();
                    self.emit(&mut tokens, TokenKind::Plus, start);
                }
                '-' => {
                    let start = self.pos;
                    self.next_char();
                    self.emit(&mut tokens, TokenKind::Minus, start);
                }
                '/' => {
                    let start = self.pos;
                    self.next_char();
                    self.emit(&mut tokens, TokenKind::Slash, start);
                }
                '.' => {
                    let is_float = self.peek_second().is_some_and(|c| c.is_ascii_digit());
                    if is_float {
                        self.lex_number(&mut tokens)?;
                    } else {
                        return Err(InkError::syntax(
                            SyntaxErrorKind::UnexpectedChar(ch),
                            Span(self.pos, self.pos + 1),
                        ));
                    }
                }
                c if c.is_alphabetic() || c == '_' => self.lex_word(&mut tokens)?,
                _ => {
                    return Err(InkError::syntax(
                        SyntaxErrorKind::UnexpectedChar(ch),
                        Span(self.pos, self.pos + 1),
                    ));
                }
            }
        }

        if let Some(start) = first_open {
            return Err(InkError::syntax(
                SyntaxErrorKind::UnclosedParenthesis,
                Span(start, self.pos),
            ));
        }

        Ok(tokens)
    }

    fn peek_second(&mut self) -> Option<char> {
        let mut clone = self.chars.clone();
        clone.next()?;
        clone.peek().copied()
    }

    fn lex_string(&mut self, tokens: &mut Vec<Token>) -> Result<(), InkError> {
        let start = self.pos;
        let quote = self.next_char().expect("peeked quote");
        let mut string = String::new();

        while let Some(ch) = self.peek() {
            if ch == quote {
                self.next_char();
                self.emit(tokens, TokenKind::String(string), start);
                return Ok(());
            }
            string.push(ch);
            self.next_char();
        }

        Err(InkError::syntax(
            SyntaxErrorKind::UnterminatedString,
            Span(start, self.pos),
        ))
    }

    fn lex_number(&mut self, tokens: &mut Vec<Token>) -> Result<(), InkError> {
        let start = self.pos;
        let kind = self.extract_number()?;
        self.emit(tokens, kind, start);
        Ok(())
    }

    fn lex_word(&mut self, tokens: &mut Vec<Token>) -> Result<(), InkError> {
        let start = self.pos;
        let mut word = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' {
                word.push(ch);
                self.next_char();
            } else {
                break;
            }
        }
        if (word == "X" || word == "x") && self.peek() == Some('\'') {
            return self.lex_blob(tokens, start);
        }
        let kind = keyword(&word).unwrap_or(TokenKind::Identifier(word));
        self.emit(tokens, kind, start);
        Ok(())
    }

    fn lex_blob(&mut self, tokens: &mut Vec<Token>, start: usize) -> Result<(), InkError> {
        self.next_char();
        let mut digits = String::new();
        while let Some(ch) = self.peek() {
            if ch == '\'' {
                self.next_char();
                if !digits.len().is_multiple_of(2) {
                    return Err(InkError::syntax(
                        SyntaxErrorKind::InvalidBlobLiteral,
                        Span(start, self.pos),
                    ));
                }
                let mut bytes = Vec::with_capacity(digits.len() / 2);
                for pair in digits.as_bytes().chunks(2) {
                    let hi = (pair[0] as char).to_digit(16);
                    let lo = (pair[1] as char).to_digit(16);
                    match (hi, lo) {
                        (Some(hi), Some(lo)) => bytes.push((hi * 16 + lo) as u8),
                        _ => {
                            return Err(InkError::syntax(
                                SyntaxErrorKind::InvalidBlobLiteral,
                                Span(start, self.pos),
                            ));
                        }
                    }
                }
                self.emit(tokens, TokenKind::BlobVar(bytes), start);
                return Ok(());
            }
            digits.push(ch);
            self.next_char();
        }
        Err(InkError::syntax(
            SyntaxErrorKind::UnterminatedString,
            Span(start, self.pos),
        ))
    }

    fn extract_number(&mut self) -> Result<TokenKind, InkError> {
        let start = self.pos;
        let mut number = String::new();
        let mut is_float = false;

        // Integer part (may be empty for leading-dot floats like .5).
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                number.push(ch);
                self.next_char();
            } else {
                break;
            }
        }

        if self.peek() == Some('.') {
            is_float = true;
            number.push('.');
            self.next_char();
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    number.push(ch);
                    self.next_char();
                } else {
                    break;
                }
            }
        }

        if matches!(self.peek(), Some('e') | Some('E')) {
            let mut probe = self.chars.clone();
            probe.next(); // e/E
            if matches!(probe.peek(), Some('+') | Some('-')) {
                probe.next();
            }
            if matches!(probe.peek(), Some(c) if c.is_ascii_digit()) {
                is_float = true;
                number.push(self.next_char().unwrap_or('e'));
                if matches!(self.peek(), Some('+') | Some('-')) {
                    number.push(self.next_char().unwrap_or('+'));
                }
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_digit() {
                        number.push(ch);
                        self.next_char();
                    } else {
                        break;
                    }
                }
            }
        }

        let invalid = |end| InkError::syntax(SyntaxErrorKind::InvalidNumber, Span(start, end));
        if is_float {
            number
                .parse::<f64>()
                .map(TokenKind::FloatVar)
                .map_err(|_| invalid(self.pos))
        } else {
            number
                .parse::<i64>()
                .map(TokenKind::NumberVar)
                .map_err(|_| invalid(self.pos))
        }
    }
}

fn keyword(word: &str) -> Option<TokenKind> {
    let upper = word.to_ascii_uppercase();
    let kind = match upper.as_str() {
        "CREATE" => TokenKind::Create,
        "TABLE" => TokenKind::Table,
        "INDEX" => TokenKind::Index,
        "DROP" => TokenKind::Drop,
        "IF" => TokenKind::If,
        "PRIMARY" => TokenKind::Primary,
        "KEY" => TokenKind::Key,
        "UNIQUE" => TokenKind::Unique,
        "CHECK" => TokenKind::Check,
        "DEFAULT" => TokenKind::Default,
        "ON" => TokenKind::On,
        "DELETE" => TokenKind::Delete,
        "UPDATE" => TokenKind::Update,
        "DESC" => TokenKind::Desc,
        "ASC" => TokenKind::Asc,
        "INSERT" => TokenKind::Insert,
        "INTO" => TokenKind::Into,
        "VALUES" => TokenKind::Values,
        "SELECT" => TokenKind::Select,
        "FROM" => TokenKind::From,
        "WHERE" => TokenKind::Where,
        "AND" => TokenKind::And,
        "OR" => TokenKind::Or,
        "SET" => TokenKind::Set,
        "BY" => TokenKind::By,
        "ORDER" => TokenKind::Order,
        "LIMIT" => TokenKind::Limit,
        "ROLLBACK" => TokenKind::RollBack,
        "NOT" => TokenKind::Not,
        "IS" => TokenKind::Is,
        "EXISTS" => TokenKind::Exists,
        "EXPLAIN" => TokenKind::Explain,
        "BEGIN" => TokenKind::Begin,
        "COMMIT" => TokenKind::Commit,
        "NOTNULL" => TokenKind::NotNull,
        "NULL" => TokenKind::Null,
        "TRUE" => TokenKind::BoolVar(true),
        "FALSE" => TokenKind::BoolVar(false),
        "BOOL" => TokenKind::Bool,
        "INT" | "INTEGER" => TokenKind::Integer,
        "TEXT" => TokenKind::Text,
        "FLOAT" | "DOUBLE" | "REAL" => TokenKind::Float,
        "BLOB" => TokenKind::Blob,
        _ => return None,
    };
    Some(kind)
}

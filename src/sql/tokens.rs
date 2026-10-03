#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub struct Span(pub usize, pub usize);

#[derive(Debug, PartialEq, PartialOrd, Clone)]
pub enum TokenKind {
    Create,
    Table,
    Index,
    Drop,
    If,
    Primary,
    Key,
    Unique,
    Check,
    Default,
    On,
    Delete,
    Update,
    Set,
    Desc,
    Explain,
    Asc,

    Insert,
    Into,
    Values,
    Select,
    From,
    Where,
    And,
    Or,
    By,
    Order,
    Limit,

    Not,
    Exists,
    NotNull,

    Identifier(String),
    BoolVar(bool),
    NumberVar(i64),
    FloatVar(f64),
    String(String),
    Blob,
    Null,

    Integer,
    Text,
    Float,
    Bool,

    Comma,
    LeftParen,
    RightParen,
    Equals,
    NotEquals,
    Ge,
    Gt,
    Le,
    Lt,
    Plus,
    Minus,
    Slash,
    Star,

    Begin,
    RollBack,
    Commit,
}
use std::fmt::{self};

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // DDL
            TokenKind::Create => write!(f, "CREATE"),
            TokenKind::Table => write!(f, "TABLE"),
            TokenKind::RollBack => write!(f, "ROLLBACK"),
            TokenKind::Index => write!(f, "INDEX"),
            TokenKind::Drop => write!(f, "DROP"),
            TokenKind::If => write!(f, "IF"),
            TokenKind::Primary => write!(f, "PRIMARY"),
            TokenKind::Key => write!(f, "KEY"),
            TokenKind::Unique => write!(f, "UNIQUE"),
            TokenKind::Check => write!(f, "CHECK"),
            TokenKind::Default => write!(f, "DEFAULT"),
            TokenKind::On => write!(f, "ON"),
            TokenKind::Delete => write!(f, "DELETE"),
            TokenKind::Begin => write!(f, "BEGIN"),
            TokenKind::Commit => write!(f, "COMMIT"),
            TokenKind::Update => write!(f, "UPDATE"),
            TokenKind::Desc => write!(f, "DESC"),
            TokenKind::Asc => write!(f, "ASC"),

            // DML
            TokenKind::Insert => write!(f, "INSERT"),
            TokenKind::Into => write!(f, "INTO"),
            TokenKind::Values => write!(f, "VALUES"),
            TokenKind::Select => write!(f, "SELECT"),
            TokenKind::From => write!(f, "FROM"),
            TokenKind::Where => write!(f, "WHERE"),
            TokenKind::Set => write!(f, "SET"),
            TokenKind::And => write!(f, "AND"),
            TokenKind::Or => write!(f, "OR"),
            TokenKind::By => write!(f, "BY"),
            TokenKind::Order => write!(f, "ORDER"),
            TokenKind::Limit => write!(f, "LIMIT"),

            // Expressions
            TokenKind::Not => write!(f, "NOT"),
            TokenKind::Exists => write!(f, "EXISTS"),
            TokenKind::NotNull => write!(f, "NOT NULL"),

            // Values / literals
            TokenKind::Identifier(_) => write!(f, "IDENTIFIER"),
            TokenKind::BoolVar(_) => write!(f, "BOOL"),
            TokenKind::NumberVar(_) => write!(f, "NUMBER"),
            TokenKind::FloatVar(_) => write!(f, "FLOAT"),
            TokenKind::String(_) => write!(f, "STRING"),
            TokenKind::Blob => write!(f, "BLOB"),
            TokenKind::Null => write!(f, "NULL"),

            // Punctuation / operators
            TokenKind::Comma => write!(f, ","),
            TokenKind::LeftParen => write!(f, "("),
            TokenKind::RightParen => write!(f, ")"),

            TokenKind::Equals => write!(f, "="),
            TokenKind::NotEquals => write!(f, "!="),
            TokenKind::Ge => write!(f, ">="),
            TokenKind::Gt => write!(f, ">"),
            TokenKind::Le => write!(f, "<="),
            TokenKind::Lt => write!(f, "<"),

            TokenKind::Plus => write!(f, "+"),
            TokenKind::Minus => write!(f, "-"),
            TokenKind::Slash => write!(f, "/"),
            TokenKind::Star => write!(f, "*"),

            // Types
            TokenKind::Integer => write!(f, "INTEGER"),
            TokenKind::Text => write!(f, "TEXT"),
            TokenKind::Float => write!(f, "FLOAT"),
            TokenKind::Bool => write!(f, "BOOL"),
            TokenKind::Explain => write!(f, "EXPLAIN"),
        }
    }
}

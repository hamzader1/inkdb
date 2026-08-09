#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]

pub enum TokenKind {
    // DDL
    Create,
    Table,
    Index,
    Drop,
    If,
    Constraint,
    Primary,
    Key,
    Unique,
    Check,
    Default,
    Without,
    Rowid,
    On,
    Delete,
    Update,
    Desc,
    Asc,

    // DML
    Insert,
    Into,
    Values,
    Select,
    From,
    Where,
    And,
    Or,
    Group,
    By,
    Having,
    Order,
    Limit,
    Distinct,
    Union,
    All,
    Join,
    Inner,
    Outer,
    Left,
    Right,
    Full,
    Cross,
    As,

    // expressions
    In,
    Between,
    Like,
    Is,
    Not,
    Exists,
    Case,
    When,
    Then,
    Else,
    End,
    Cast,
    IsNull,
    NotNull,

    // values / literals
    Identifier(String),
    Number(String), // integer or float literal
    String(String), // 'quoted'
    Blob(String),
    Null,

    // punctuation / operators
    Semicolon,
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
    Astrisk,
    Modulus,
    Concat,
    BitAnd,
    BitOr,
    ShiftLeft,
    ShiftRight,
    Dot,
    Tilde,
    EmptyIdentifier,
}

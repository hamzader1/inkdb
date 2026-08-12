use super::tokens::TokenKind;

#[derive(Debug)]
pub struct CreateTable {
    pub name: String,
    pub columns: Vec<Column>,
}

#[derive(Debug)]
pub struct CreateIndex {
    pub unique: bool,
    pub name: String,
    pub table: String,
    pub columns: Vec<String>,
}

#[derive(Debug)]
pub struct Column {
    pub name: String,
    pub affinity: Affinity,
    pub constraints: Option<Vec<Constraint>>,
}
#[derive(Debug, Clone)]
pub enum Expr {
    Empty,
    Number(i64),
    Float(f64),
    StringLitteral(String),
    Bool(bool),
    Identifier(String),
    Add(Box<Expr>, Box<Expr>),
    Substract(Box<Expr>, Box<Expr>),
    Devide(Box<Expr>, Box<Expr>),
    Multiply(Box<Expr>, Box<Expr>),
    BinaryOp {
        left: Box<Expr>,
        op: BinaryOperator,
        right: Box<Expr>,
    },

    And {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    Or {
        left: Box<Expr>,
        right: Box<Expr>,
    },
}
#[derive(Debug, Clone)]
pub enum BinaryOperator {
    Eq,
    NotEq,
    Ge,
    Le,
    Gt,
    Lt,
    Contains,
}

pub struct SelectStmt {
    table_name: String,
    columns: Vec<Column>,
    where_clause: Option<Expr>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    Text,
    Float,
    Int,
    Blob,
}

impl Affinity {
    pub fn from_type_name(name: &str) -> Self {
        let upper = name.to_uppercase();
        if upper.contains("INT") {
            Self::Int
        } else if upper.contains("CHAR") || upper.contains("CLOB") || upper.contains("TEXT") {
            Self::Text
        } else if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") {
            Self::Float
        } else {
            Self::Blob
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Constraint {
    PrimaryKey,
    NotNull,
    Unique,
}

#[derive(Debug)]
pub enum Ast {
    CreateTableAst(CreateTable),
    CreateIndexAst(CreateIndex),
}

impl From<TokenKind> for Affinity {
    fn from(value: TokenKind) -> Self {
        match value {
            TokenKind::Integer => Self::Int,
            TokenKind::Text => Self::Text,
            TokenKind::Float => Self::Float,
            TokenKind::Blob => Self::Blob,
            _ => unreachable!(),
        }
    }
}

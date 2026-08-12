use super::parser::Arena;
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
    Number(i64),
    Float(f64),
    StringLitteral(String),
    Bool(bool),
    ColumnRef(usize), // used only by select_v2
    Identifier(String),
    Add(usize, usize),
    Substract(usize, usize),
    Devide(usize, usize),
    Multiply(usize, usize),
    Neg(usize),
    Not(usize),

    BinaryOp {
        left: usize,
        op: BinaryOperator,
        right: usize,
    },

    And {
        left: usize,
        right: usize,
    },

    Or {
        left: usize,
        right: usize,
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

#[derive(Debug)]
pub struct SelectStmt {
    pub table_name: String,
    pub arena: Arena,
    pub columns: Vec<usize>,
    pub where_clause: Option<usize>, // same arena used twice
}

// #[derive(Debqug)]
// pub struct WhereClause {
//     arena: Arena,
//     expr_idx: usize,
// }
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
    SelectStmtAst(SelectStmt),
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

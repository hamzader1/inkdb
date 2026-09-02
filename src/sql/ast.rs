use crate::record::Value;

use super::parser::ExprArena;
use super::tokens::TokenKind;
use std::rc::Rc;

#[derive(Debug)]
pub struct CreateTable {
    pub query: Rc<str>,
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
#[derive(Debug, Clone, PartialEq)]
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
    Star,

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
impl Expr {
    pub fn remap_l_r(expr: &Expr, l: usize, r: usize) -> Expr {
        match expr {
            Expr::Add(_, _) => Expr::Add(l, r),
            Expr::Substract(_, _) => Expr::Substract(l, r),
            Expr::Multiply(_, _) => Expr::Multiply(l, r),
            Expr::Devide(_, _) => Expr::Devide(l, r),
            Expr::And { left: _, right: _ } => Expr::And { left: l, right: r },
            Expr::Or { left: _, right: _ } => Expr::Or { left: l, right: r },
            Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
                left: l,
                op: op.clone(),
                right: r,
            },
            _ => panic!("Reached unmapped Expression"),
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum BinaryOperator {
    Eq,
    NotEq,
    Ge,
    Le,
    Gt,
    Lt,
}

#[derive(Debug)]
pub struct SelectStmt {
    pub table_name: String,
    pub arena: ExprArena,
    pub columns: Vec<usize>,
    pub where_clause: Option<usize>, // same arena used twice
    pub limit: Option<usize>,
}

#[derive(Debug)]
pub struct InsertStmt {
    pub table_name: String,
    pub columns: Vec<String>,
    pub values: Vec<Vec<Value<'static>>>,
}

#[derive(Debug)]
pub struct DeleteStmt {
    pub(crate) table_name: String,
    pub(crate) arena: Option<ExprArena>,
    pub(crate) where_clause: Option<usize>,
}

// pub enum QueryStmt {
//     Select(SelectStmt),
//     Insert(InsertStmt),
// }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    Text,
    Float,
    Int,
    Blob,
}
impl<'a> From<&Value<'a>> for Affinity {
    fn from(value: &Value) -> Self {
        match value {
            Value::Integer(_) => Affinity::Int,
            Value::Float(_) => Affinity::Float,
            Value::Text(_) => Affinity::Text,
            Value::Blob(_) => Affinity::Blob,
            _ => unreachable!(),
        }
    }
}
impl std::fmt::Display for Affinity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blob => write!(f, "Blob"),
            Self::Text => write!(f, "Text"),
            Self::Int => write!(f, "Int"),
            Self::Float => write!(f, "Float"),
        }
    }
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
    InsertStmtAst(InsertStmt),
    DeleteStmtAst(DeleteStmt),
    BeginTransaction,
    CommitTransaction,
    RollbackTransaction,
}

impl From<TokenKind> for Affinity {
    fn from(value: TokenKind) -> Self {
        match value {
            TokenKind::Integer | TokenKind::Bool => Self::Int,
            TokenKind::Text => Self::Text,
            TokenKind::Float => Self::Float,
            TokenKind::Blob => Self::Blob,
            _ => unreachable!(),
        }
    }
}

use std::fmt;

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Number(n) => write!(f, "{n}"),
            Expr::Float(n) => write!(f, "{n}"),
            Expr::StringLitteral(s) => write!(f, "'{s}'"),
            Expr::Bool(b) => write!(f, "{b}"),
            Expr::ColumnRef(idx) => write!(f, "column[{idx}]"),
            Expr::Identifier(s) => write!(f, "{s}"),

            Expr::Add(left, right) => write!(f, "({left} + {right})"),
            Expr::Substract(left, right) => write!(f, "({left} - {right})"),
            Expr::Devide(left, right) => write!(f, "({left} / {right})"),
            Expr::Multiply(left, right) => write!(f, "({left} * {right})"),

            Expr::Neg(expr) => write!(f, "-{expr}"),
            Expr::Not(expr) => write!(f, "NOT {expr}"),
            Expr::Star => write!(f, "*"),

            Expr::BinaryOp { left, op, right } => {
                write!(f, "({left} {op} {right})")
            }

            Expr::And { left, right } => {
                write!(f, "({left} AND {right})")
            }

            Expr::Or { left, right } => {
                write!(f, "({left} OR {right})")
            }
        }
    }
}

impl fmt::Display for BinaryOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op = match self {
            BinaryOperator::Eq => "=",
            BinaryOperator::NotEq => "!=",
            BinaryOperator::Ge => ">=",
            BinaryOperator::Le => "<=",
            BinaryOperator::Gt => ">",
            BinaryOperator::Lt => "<",
        };

        write!(f, "{op}")
    }
}

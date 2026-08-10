use super::tokens::TokenKind;

pub struct CreateTable {
    name: String,
    columns: Vec<Column>,
}
pub struct Column {
    name: String,
    affinity: Affinity,
    constraints: Option<Vec<Constraint>>,
}

pub enum Affinity {
    Text,
    Float,
    Int,
    Blob,
}
pub enum Constraint {
    PrimaryKey,
    NotNull,
    Unique,
}

pub enum Ast {
    CreateTable { name: String, columns: Vec<Column> },
    Null,
}

impl From<TokenKind> for Affinity {
    fn from(value: TokenKind) -> Self {
        match value {
           TokenKind::Integer => Self::Int,
           TokenKind::Text => Self::Text,
           TokenKind::Float => Self::Float,
           TokenKind::Blob => Self::Blob,
           _ => unreachable!()
        }
    }
}

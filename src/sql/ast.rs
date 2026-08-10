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

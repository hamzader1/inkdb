use crate::SqliteMaster;
use crate::errors::SqliteError;
use crate::schema::Table;
use crate::sql::ast::{Expr, SelectStmt};
use crate::sql::parser::Arena;

type ColumnIndex = usize;
type ArenaColumnIndex = usize;

pub struct Analayze;
#[derive(Debug)]
pub struct MultiIndexColumn {
    pub col_idx: ColumnIndex,
    pub arena_col_idx: ArenaColumnIndex,
}
#[derive(Debug)]
pub struct ResolvedQuery {
    pub root_page: u32,
    pub arena: Arena,
    pub columns: Vec<MultiIndexColumn>,
    pub where_clause: Option<usize>,
}

impl MultiIndexColumn {
    fn new(col_idx: usize, arena_col_idx: usize) -> Self {
        Self {
            col_idx,
            arena_col_idx,
        }
    }
}

impl Analayze {
    pub fn analyze(
        select_stmt: SelectStmt,
        sqlite_master: &SqliteMaster,
    ) -> Result<ResolvedQuery, SqliteError> {
        let SelectStmt {
            table_name,
            mut arena,
            columns,
            where_clause,
        } = select_stmt;
        let table = match sqlite_master.tables.get(&table_name) {
            Some(table) => table,
            _ => {
                return Err(SqliteError::RuntimeError(format!(
                    "Table {} does not exists",
                    table_name
                )));
            }
        };
        let mut multi_index_columns = Vec::new();
        for idx in columns.iter() {
            Analayze::bind(table, *idx, *idx, &mut arena, &mut multi_index_columns)?;
        }

        Ok(ResolvedQuery {
            root_page: table.root_page,
            arena,
            columns: multi_index_columns,
            where_clause,
        })
    }

    fn bind(
        table: &Table,
        idx: usize,
        root_idx: usize,
        arena: &mut Arena,
        columns: &mut Vec<MultiIndexColumn>,
    ) -> Result<(), SqliteError> {
        let expr = &arena.nodes[idx];
        match expr {
            Expr::Identifier(col_name) => match table.get_col_idx(col_name) {
                Some(col_idx) => {
                    arena.nodes[idx] = Expr::ColumnRef(col_idx);
                    columns.push(MultiIndexColumn::new(col_idx, root_idx));
                    return Ok(());
                }
                _ => {
                    return Err(SqliteError::RuntimeError(format!(
                        "Column {} does not exists",
                        col_name
                    )));
                }
            },
            Expr::Number(_) | Expr::Float(_) | Expr::Bool(_) | Expr::StringLitteral(_) => {
                return Ok(());
            }
            Expr::Add(left, right) => {
                let left = *left;
                let right = *right;
                Self::bind(table, left, root_idx, arena, columns)?;
                Self::bind(table, right, root_idx, arena, columns)?;
            }
            Expr::Substract(left, right) => {
                let left = *left;
                let right = *right;
                Self::bind(table, left, root_idx, arena, columns)?;
                Self::bind(table, right, root_idx, arena, columns)?;
            }
            Expr::Multiply(left, right) => {
                let left = *left;
                let right = *right;
                Self::bind(table, left, root_idx, arena, columns)?;
                Self::bind(table, right, root_idx, arena, columns)?;
            }
            Expr::Devide(left, right) => {
                let left = *left;
                let right = *right;
                Self::bind(table, left, root_idx, arena, columns)?;
                Self::bind(table, right, root_idx, arena, columns)?;
            }
            // TODO: Temporary for now, Remove in where clause
            _ => {
                return Err(SqliteError::RuntimeError(
                    "Expression is not allowed in select statement list".into(),
                ));
            }
        }
        Ok(())
    }
}

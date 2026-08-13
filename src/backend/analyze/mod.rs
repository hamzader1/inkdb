use crate::SqliteMaster;
use crate::errors::SqliteError;
use crate::schema::Table;
use crate::sql::ast::{Expr, SelectStmt};
use crate::sql::parser::Arena;

type ColumnIndex = usize;
type ArenaColumnIndex = usize;

pub struct Analayze;
// #[derive(Debug)]
// pub struct MultiIndexColumn {
//     pub col_idx: ColumnIndex,
//     pub arena_col_idx: ArenaColumnIndex,
// }
#[derive(Debug)]
pub struct ResolvedQuery {
    pub root_page: u32,
    pub arena: Arena,
    pub columns: Vec<usize>,
    pub where_clause: Option<usize>,
}

// impl MultiIndexColumn {
//     fn new(col_idx: usize, arena_col_idx: usize) -> Self {
//         Self {
//             col_idx,
//             arena_col_idx,
//         }
//     }
// }

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
        for idx in columns.iter() {
            Analayze::bind(table, *idx, &mut arena)?;
        }

        Ok(ResolvedQuery {
            root_page: table.root_page,
            arena,
            columns,
            where_clause,
        })
    }

    fn bind(table: &Table, idx: usize, arena: &mut Arena) -> Result<(), SqliteError> {
        let expr = &arena.nodes[idx];
        match expr {
            Expr::Identifier(col_name) => match table.get_col_idx(col_name) {
                Some(col_idx) => {
                    arena.nodes[idx] = Expr::ColumnRef(col_idx);
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
                Self::bind(table, left, arena)?;
                Self::bind(table, right, arena)?;
            }
            Expr::Substract(left, right) => {
                let left = *left;
                let right = *right;
                Self::bind(table, left, arena)?;
                Self::bind(table, right, arena)?;
            }
            Expr::Multiply(left, right) => {
                let left = *left;
                let right = *right;
                Self::bind(table, left, arena)?;
                Self::bind(table, right, arena)?;
            }
            Expr::Devide(left, right) => {
                let left = *left;
                let right = *right;
                Self::bind(table, left, arena)?;
                Self::bind(table, right, arena)?;
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

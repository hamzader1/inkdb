use crate::SqliteMaster;
use crate::errors::SqliteError;
use crate::schema::Table;
use crate::sql::ast::{Expr, SelectStmt};
use crate::sql::parser::Arena;

pub struct Plan;
pub struct BoundSelect {
    root_page: u32,
    arena: Arena,
    columns: Vec<usize>,
    where_clause: Option<usize>,
}

impl Plan {
    pub fn plan(
        select_stmt: SelectStmt,
        sqlite_master: &SqliteMaster,
    ) -> Result<BoundSelect, SqliteError> {
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
        // columns.iter().map(|i| Plan::bind(table, *i, &mut arena));
        for idx in columns.iter() {
            Plan::bind(table, *idx, &mut arena)?;
        }

        Ok(BoundSelect {
            root_page: table.root_page,
            arena,
            columns,
            where_clause,
        })
    }

    fn bind(table: &Table, idx: usize, arena: &mut Arena) -> Result<(), SqliteError> {
        match &mut arena.nodes[idx] {
            Expr::Identifier(col_name) => match table.get_col_idx(col_name) {
                Some(col_idx) => arena.nodes[idx] = Expr::ColumnRef(col_idx),
                _ => {
                    return Err(SqliteError::RuntimeError(format!(
                        "Column {} does not exists",
                        col_name
                    )));
                }
            },
            Expr::Number(_) | Expr::Float(_) | Expr::Bool(_) | Expr::StringLitteral(_) => {}
            Expr::Add(left, right) => Self::bind(table, idx, arena)?,
            Expr::Substract(left, right) => Self::bind(table, idx, arena)?,
            Expr::Multiply(left, right) => Self::bind(table, idx, arena)?,
            Expr::Devide(left, right) => Self::bind(table, idx, arena)?,
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

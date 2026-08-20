use crate::SqliteMaster;
use crate::errors::SqliteError;
use crate::format::page::PageNo;
use crate::record::Value;
use crate::schema::Table;
use crate::sql::ast::{Affinity, Ast, Expr, InsertStmt, SelectStmt};
use crate::sql::parser::ExprArena;
use crate::util::{sqlite_assert_one, sqlite_assert_with_corrupt_err};
pub mod bind;
pub mod insert;
pub mod select;

type ColumnIndex = usize;
type ArenaColumnIndex = usize;

pub struct Analyze;
#[derive(Debug)]
pub struct ResolvedSelectQuery {
    pub root_page: u32,
    pub arena: ExprArena,
    pub columns: Vec<usize>,
    pub where_clause: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug)]
pub struct ResolvedInsertQuery {
    pub root: PageNo,
    pub values: Vec<Value<'static>>,
    pub entry_hint: Option<u64>, // row id hint
}

#[derive(Debug)]
pub enum ResolvedQuery {
    SelectQuery(ResolvedSelectQuery),
    InsertQuery(ResolvedInsertQuery),
}

impl Analyze {
    pub fn analyze(stmt: Ast, sqlite_master: &SqliteMaster) -> Result<ResolvedQuery, SqliteError> {
        match stmt {
            Ast::SelectStmtAst(select_stmt) => {
                Self::analyze_select_stmt(select_stmt, sqlite_master)
            }
            Ast::InsertStmtAst(insert_stmt) => {
                Self::analyze_insert_stmt(insert_stmt, sqlite_master)
            }
            _ => todo!(),
        }
    }

    pub fn get_table<'s>(
        sqlite_master: &'s SqliteMaster,
        table_name: &str,
    ) -> Result<&'s Table, SqliteError> {
        let table = match sqlite_master.tables.get(&table_name.to_lowercase()) {
            Some(table) => table,
            _ => {
                return Err(SqliteError::RuntimeError(format!(
                    "Table {} does not exists",
                    table_name
                )));
            }
        };
        Ok(table)
    }
}

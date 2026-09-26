use crate::SqliteMaster;
use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::pager::PageNo;
use crate::record::Value;
use crate::schema::Table;
use crate::sql::ast::{Ast, CreateTable};
use crate::sql::parser::ExprArena;
pub mod bind;
pub mod create;
pub(crate) mod delete;
pub mod insert;
pub mod select;

pub struct Analyze;
#[derive(Debug)]
pub struct ResolvedSelectQuery {
    pub table_name: String,
    pub root_page: u32,
    pub arena: ExprArena,
    pub columns: Vec<usize>,
    pub where_clause: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug)]
pub struct ResolvedInsertQuery {
    pub table_name: String,
    pub root_page: PageNo,
    pub values: Vec<Vec<Value<'static>>>,
}

#[derive(Debug, Copy, Clone)]
pub struct IndexMetadata {
    pub index_root_page: u32,
    pub col_idx: usize,
    pub is_unique: bool,
}
impl IndexMetadata {
    pub fn new(index_root_page: u32, col_idx: usize, is_unique: bool) -> Self {
        Self {
            index_root_page,
            col_idx,
            is_unique,
        }
    }

    pub fn key_for(&self, values: &[Value<'static>], rowid: u64) -> Vec<Value<'static>> {
        vec![values[self.col_idx].clone(), Value::Integer(rowid as i64)]
    }
}

pub fn rowid_of(entry: &[Value<'_>]) -> SqliteResult<u64> {
    entry
        .last()
        .ok_or_else(|| SqliteError::runtime("index entry is empty"))?
        .cast_int()
        .map(|rowid| rowid as u64)
}
#[derive(Debug)]
pub struct ResolvedCreateTableQuery {
    pub meta: CreateTable,
}

#[derive(Debug)]
pub struct ResolvedDeleteQuery {
    pub table_name: String,
    pub root_page: PageNo,
    pub arena: Option<ExprArena>,
    pub where_clause: Option<usize>,
}

#[derive(Debug)]
pub struct ResolvedTruncateTableQuery {
    pub table_name: String,
    pub root_page: u32,
}

use std::rc::Rc;
#[derive(Debug)]
pub struct ResolvedCreateIndexQuery {
    pub query: Rc<str>,
    pub relation_root_page: u32,
    pub relation_name: String,
    pub index_name: String,
    pub column_index: usize, // todo: usize -> Vec::<usize>
}

#[derive(Debug)]
pub struct ResolvedExplainQuery {
    pub query: Box<ResolvedQuery>,
}

#[derive(Debug)]
pub enum ResolvedQuery {
    SelectQuery(ResolvedSelectQuery),
    InsertQuery(ResolvedInsertQuery),
    CreateTableQuery(ResolvedCreateTableQuery),
    CreateIndexQuery(ResolvedCreateIndexQuery),
    DeleteQuery(ResolvedDeleteQuery),
    TruncateTable(ResolvedTruncateTableQuery),
    BeginTransactionQuery,
    CommitTransactionQuery,
    RollbackTransactionQuery,
    ExplainQuery(ResolvedExplainQuery),
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
            Ast::CreateTableAst(create_stmt) => {
                Self::analyze_create_table_stmt(create_stmt, sqlite_master)
            }
            Ast::DeleteStmtAst(delete_stmt) => {
                Self::analyze_delete_stmt(delete_stmt, sqlite_master)
            }
            Ast::BeginTransaction => Ok(ResolvedQuery::BeginTransactionQuery),
            Ast::CommitTransaction => Ok(ResolvedQuery::CommitTransactionQuery),
            Ast::RollbackTransaction => Ok(ResolvedQuery::RollbackTransactionQuery),
            Ast::TruncateTableAst(t_stmt) => {
                let table = Self::get_table(sqlite_master, &t_stmt.table_name)?;
                Ok(ResolvedQuery::TruncateTable(ResolvedTruncateTableQuery {
                    table_name: table.name.clone(),
                    root_page: table.root_page,
                }))
            }
            Ast::CreateIndexAst(ci_stmt) => Self::analyze_create_index_stmt(ci_stmt, sqlite_master),
            Ast::ExplainStmtAst(stmt) => Ok(ResolvedQuery::ExplainQuery(ResolvedExplainQuery {
                query: Box::new(Self::analyze(*stmt.query, sqlite_master)?),
            })),
        }
    }

    pub fn get_table<'s>(
        sqlite_master: &'s SqliteMaster,
        table_name: &str,
    ) -> Result<&'s Table, SqliteError> {
        sqlite_master
            .table(table_name)
            .ok_or_else(|| SqliteError::TableNotFound(table_name.to_string()))
    }
}

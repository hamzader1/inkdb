use crate::InkResult;
use crate::Master;
use crate::errors::InkError;
use crate::pager::pager::PageNo;
use crate::record::{Record, Value};
use crate::schema::MASTER;
use crate::schema::Table;
use crate::sql::ast::OrderBy;
use crate::sql::ast::{Ast, CreateTableStmt};
use crate::sql::parser::ExprArena;
use crate::util::assert_with_runtime_err;
pub mod bind;
pub mod create;
pub(crate) mod delete;
pub mod insert;
pub mod select;
pub mod update;

pub struct Analyze<'a> {
    master: &'a Master,
}
#[derive(Debug)]
pub struct ResolvedSelectQuery {
    pub table_name: String,
    pub root_page: u32,
    pub arena: ExprArena,
    pub columns: Vec<usize>,
    pub where_clause: Option<usize>,
    pub limit: Option<usize>,
    pub orderby: Option<OrderBy>,
}

#[derive(Debug)]
pub struct ResolvedInsertQuery {
    pub table_name: String,
    pub root_page: PageNo,
    pub values: Vec<Vec<Value<'static>>>,
}

impl ResolvedInsertQuery {
    pub fn new(table_name: String, root_page: PageNo, values: Vec<Vec<Value<'static>>>) -> Self {
        Self {
            table_name,
            root_page,
            values,
        }
    }
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

    pub fn key_for(&self, value: Value<'static>, rowid: u64) -> Vec<Value<'static>> {
        vec![value, Value::Integer(rowid as i64)]
    }
}

pub fn rowid_of(entry: &Record<'_>) -> InkResult<u64> {
    match entry.last() {
        Some(rowid) => rowid?.cast_int().map(|rowid| rowid as u64),
        None => Err(InkError::runtime("index entry is empty")),
    }
}
#[derive(Debug)]
pub struct ResolvedCountQuery {
    pub table_name: String,
    pub root_page: u32,
    pub arena: ExprArena,
    pub arg: Option<usize>,
    pub where_clause: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug)]
pub struct ResolvedCreateTableQuery {
    pub meta: CreateTableStmt,
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
    pub is_unique: bool,
}
#[derive(Debug)]
pub struct ResolvedUpdateQuery {
    pub table_name: String,
    pub root_page: u32,
    pub affected_columns: Vec<(usize, usize)>,
    pub where_clause: Option<usize>,
    pub arena: ExprArena,
}

impl ResolvedUpdateQuery {
    pub fn new(
        table_name: String,
        root_page: u32,
        affected_columns: Vec<(usize, usize)>,
        where_clause: Option<usize>,
        arena: ExprArena,
    ) -> Self {
        Self {
            table_name,
            root_page,
            affected_columns,
            where_clause,
            arena,
        }
    }
}

#[derive(Debug)]
pub struct ResolvedExplainQuery {
    pub query: Box<ResolvedQuery>,
}

#[derive(Debug)]
pub enum ResolvedQuery {
    SelectQuery(ResolvedSelectQuery),
    CountQuery(ResolvedCountQuery),
    InsertQuery(ResolvedInsertQuery),
    CreateTableQuery(ResolvedCreateTableQuery),
    CreateIndexQuery(ResolvedCreateIndexQuery),
    UpdateQuery(ResolvedUpdateQuery),
    DeleteQuery(ResolvedDeleteQuery),
    TruncateTable(ResolvedTruncateTableQuery),
    BeginTransactionQuery,
    CommitTransactionQuery,
    RollbackTransactionQuery,
    ExplainQuery(ResolvedExplainQuery),
}

impl<'a> Analyze<'a> {
    pub fn new(master: &'a Master) -> Self {
        Self { master }
    }

    pub fn analyze(&self, stmt: Ast) -> Result<ResolvedQuery, InkError> {
        match stmt {
            Ast::SelectStmtAst(select_stmt) => self.analyze_select_stmt(select_stmt),
            Ast::InsertStmtAst(insert_stmt) => self.analyze_insert_stmt(insert_stmt),
            Ast::CreateTableAst(create_stmt) => self.analyze_create_table_stmt(create_stmt),
            Ast::DeleteStmtAst(delete_stmt) => self.analyze_delete_stmt(delete_stmt),
            Ast::BeginTransaction => Ok(ResolvedQuery::BeginTransactionQuery),
            Ast::CommitTransaction => Ok(ResolvedQuery::CommitTransactionQuery),
            Ast::RollbackTransaction => Ok(ResolvedQuery::RollbackTransactionQuery),
            Ast::TruncateTableAst(t_stmt) => {
                let table = self.get_non_master_table(&t_stmt.table_name)?;
                Ok(ResolvedQuery::TruncateTable(ResolvedTruncateTableQuery {
                    table_name: table.name.clone(),
                    root_page: table.root_page,
                }))
            }
            Ast::CreateIndexAst(ci_stmt) => self.analyze_create_index_stmt(ci_stmt),
            Ast::ExplainStmtAst(stmt) => Ok(ResolvedQuery::ExplainQuery(ResolvedExplainQuery {
                query: Box::new(self.analyze(*stmt.query)?),
            })),
            Ast::UpdateStmtAst(update_stmt) => self.analyze_update_stmt(update_stmt),
            _ => todo!(),
        }
    }

    pub fn get_table(&'a self, table_name: &str) -> Result<&'a Table, InkError> {
        if table_name.eq_ignore_ascii_case("master") {
            return Ok(&MASTER);
        }
        self.master
            .table(table_name)
            .ok_or_else(|| InkError::TableNotFound(table_name.to_string()))
    }
    pub fn get_non_master_table(&self, table_name: &str) -> Result<&'a Table, InkError> {
        assert_with_runtime_err(!table_name.eq_ignore_ascii_case("master"), || {
            InkError::MasterTableError.to_string()
        })?;
        self.master
            .table(table_name)
            .ok_or_else(|| InkError::TableNotFound(table_name.to_string()))
    }
}

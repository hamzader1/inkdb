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
pub mod drop;
pub mod insert;
pub mod select;
pub mod update;

pub struct Analyze<'a> {
    master: &'a Master,
}
#[derive(Debug)]
pub struct ResolvedSelectQuery {
    pub(crate) table_name: String,
    pub(crate) root_page: u32,
    pub(crate) arena: ExprArena,
    pub(crate) columns: Box<[usize]>,
    pub(crate) where_clause: Option<usize>,
    pub(crate) limit: Option<usize>,
    pub(crate) orderby: Option<OrderBy>,
}

#[derive(Debug)]
pub struct ResolvedInsertQuery {
    pub(crate) table_name: String,
    pub(crate) root_page: PageNo,
    pub(crate) values: Vec<Vec<Value<'static>>>,
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

#[derive(Debug)]
pub struct ResolvedDropTableQuery {
    pub(crate) root_page: u32,
    pub(crate) tbl_name: String,
    pub(crate) indexes: Vec<u32>,
}
#[derive(Debug)]
pub struct ResolvedDropIndexQuery {
    pub(crate) index_name: String,
    pub(crate) root_page: u32,
}
#[derive(Debug, Copy, Clone)]
pub(crate) struct IndexMetadata {
    pub(crate) index_root_page: u32,
    pub(crate) col_idx: usize,
    pub(crate) is_unique: bool,
}
impl IndexMetadata {
    pub fn new(index_root_page: u32, col_idx: usize, is_unique: bool) -> Self {
        Self {
            index_root_page,
            col_idx,
            is_unique,
        }
    }

    pub fn key_for(&self, value: Value<'static>, rowid: u64) -> Box<[Value<'static>]> {
        [value, Value::Integer(rowid as i64)].into()
    }
}

pub(crate) fn rowid_of(entry: &Record<'_>) -> InkResult<u64> {
    match entry.last() {
        Some(rowid) => rowid?.cast_int().map(|rowid| rowid as u64),
        None => Err(InkError::runtime("index entry is empty")),
    }
}
#[derive(Debug)]
pub struct ResolvedCountQuery {
    pub(crate) table_name: String,
    pub(crate) root_page: u32,
    pub(crate) arena: ExprArena,
    pub(crate) arg: Option<usize>,
    pub(crate) where_clause: Option<usize>,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug)]
pub struct ResolvedCreateTableQuery {
    pub(crate) meta: CreateTableStmt,
    pub(crate) unique_on: Vec<usize>,
}

#[derive(Debug)]
pub struct ResolvedDeleteQuery {
    pub(crate) table_name: String,
    pub(crate) root_page: PageNo,
    pub(crate) arena: Option<ExprArena>,
    pub(crate) where_clause: Option<usize>,
}

#[derive(Debug)]
pub struct ResolvedTruncateTableQuery {
    pub(crate) table_name: String,
    pub(crate) root_page: u32,
}

use std::rc::Rc;
#[derive(Debug)]
pub struct ResolvedCreateIndexQuery {
    pub(crate) query: Option<Rc<str>>,
    pub(crate) relation_root_page: u32,
    pub(crate) relation_name: String,
    pub(crate) index_name: String,
    pub(crate) column_index: usize, // todo: usize -> Vec::<usize>
    pub(crate) is_unique: bool,
}

impl ResolvedCreateIndexQuery {
    pub fn new(
        query: Option<Rc<str>>,
        relation_root_page: u32,
        relation_name: String,
        index_name: String,
        column_index: usize,
        is_unique: bool,
    ) -> Self {
        Self {
            query,
            relation_root_page,
            relation_name,
            index_name,
            column_index,
            is_unique,
        }
    }
}
#[derive(Debug)]
pub struct ResolvedUpdateQuery {
    pub(crate) table_name: String,
    pub(crate) root_page: u32,
    pub(crate) affected_columns: Box<[(usize, usize)]>,
    pub(crate) where_clause: Option<usize>,
    pub(crate) arena: ExprArena,
}

impl ResolvedUpdateQuery {
    pub fn new(
        table_name: String,
        root_page: u32,
        affected_columns: Box<[(usize, usize)]>,
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
    pub(crate) query: Box<ResolvedQuery>,
}

#[derive(Debug)]
pub enum ResolvedQuery {
    SelectQuery(ResolvedSelectQuery),
    CountQuery(ResolvedCountQuery),
    InsertQuery(ResolvedInsertQuery),
    CreateTableQuery(ResolvedCreateTableQuery),
    DropTblQuery(ResolvedDropTableQuery),
    DropIndexQuery(ResolvedDropIndexQuery),
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
                    table_name: table.name().clone(),
                    root_page: table.root_page(),
                }))
            }
            Ast::CreateIndexAst(ci_stmt) => self.analyze_create_index_stmt(ci_stmt),
            Ast::ExplainStmtAst(stmt) => Ok(ResolvedQuery::ExplainQuery(ResolvedExplainQuery {
                query: Box::new(self.analyze(*stmt.query)?),
            })),
            Ast::UpdateStmtAst(update_stmt) => self.analyze_update_stmt(update_stmt),
            Ast::DropTblAst(stmt) => self.analyze_drop_tbl(stmt),
            Ast::DropIndexAst(stmt) => self.analyze_drop_index(stmt),
        }
    }

    pub(crate) fn get_table(&'a self, table_name: &str) -> Result<&'a Table, InkError> {
        if table_name.eq_ignore_ascii_case("master") {
            return Ok(&MASTER);
        }
        self.master
            .table(table_name)
            .ok_or_else(|| InkError::TableNotFound(table_name.to_string()))
    }
    pub(crate) fn get_non_master_table(&self, table_name: &str) -> Result<&'a Table, InkError> {
        assert_with_runtime_err(!table_name.eq_ignore_ascii_case("master"), || {
            InkError::MasterTableError.to_string()
        })?;
        self.master
            .table(table_name)
            .ok_or_else(|| InkError::TableNotFound(table_name.to_string()))
    }
}

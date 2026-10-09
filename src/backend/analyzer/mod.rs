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
pub mod bind;
pub mod create;
pub(crate) mod delete;
pub mod drop;
pub mod insert;
pub mod select;
pub mod update;

/// Turns a parsed statement into a resolved one.
///
/// Name binding, table lookup and constant folding all happen here, so the
/// planner and the executor only deal with arena indices and known root pages.
pub struct Analyze<'a> {
    master: &'a Master,
}
/// A SELECT that has been checked, with every name it mentions replaced by a column index.
#[derive(Debug)]
pub struct ResolvedSelectQuery {
    /// The table and its root page, or nothing when there is no FROM clause.
    pub(crate) table: Option<(String, u32)>,
    pub(crate) arena: ExprArena,
    /// One arena index per output column, in the order they were written.
    pub(crate) columns: Box<[usize]>,
    /// The predicate, as an arena index.
    pub(crate) where_clause: Option<usize>,
    /// The LIMIT expression, as an arena index. It is evaluated before the plan is built.
    pub(crate) limit: Option<usize>,
    /// The ORDER BY clause, with its expression bound to a column index.
    pub(crate) orderby: Option<OrderBy>,
}

/// An INSERT whose values have all been evaluated and checked against the column affinities, the defaults and the CHECK constraints.
#[derive(Debug)]
pub struct ResolvedInsertQuery {
    /// The table being inserted into.
    pub(crate) table_name: String,
    /// The table root page.
    pub(crate) root_page: PageNo,
    /// One row per VALUES tuple, already in table column order, so a column the
    /// statement left out is filled with its default here.
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

/// A DROP TABLE, with the pages that go with it.
#[derive(Debug)]
pub struct ResolvedDropTableQuery {
    /// The table root page.
    pub(crate) root_page: u32,
    /// The table name, matched against the catalog row to find and delete it.
    pub(crate) tbl_name: String,
    /// The root page of every index on the table, emptied and freed as well.
    pub(crate) indexes: Vec<u32>,
}
/// A DROP INDEX.
#[derive(Debug)]
pub struct ResolvedDropIndexQuery {
    /// The index name, matched against the catalog row.
    pub(crate) index_name: String,
    /// The index root page, freed once the catalog row is gone.
    pub(crate) root_page: u32,
}
/// The parts of an index a query needs once it is resolved. Kept small and copyable, since the planner hands one to each index operator it puts in a plan.
#[derive(Debug, Copy, Clone)]
pub(crate) struct IndexMetadata {
    /// The index root page.
    pub(crate) index_root_page: u32,
    /// The table column the index covers.
    pub(crate) col_idx: usize,
    /// Whether the index is unique, so an insert has to check for a clash.
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
/// A SELECT count(...).
#[derive(Debug)]
pub struct ResolvedCountQuery {
    /// The table being counted.
    pub(crate) table_name: String,
    /// Its root page.
    pub(crate) root_page: u32,
    pub(crate) arena: ExprArena,
    /// The expression to count, or nothing for count(*), which counts every row
    /// while count(x) skips the ones where x is NULL.
    pub(crate) arg: Option<usize>,
    /// The predicate rows have to pass before they are counted.
    pub(crate) where_clause: Option<usize>,
    /// The LIMIT expression, applied to the count itself.
    pub(crate) limit: Option<usize>,
}

/// A CREATE TABLE, with the statement ready to be written into the catalog.
#[derive(Debug)]
pub struct ResolvedCreateTableQuery {
    /// The table definition. Its defaults have been evaluated and its table
    /// constraints bound, and the whole statement is kept because its text goes
    /// into the catalog as the table DDL.
    pub(crate) meta: CreateTableStmt,
    /// The columns declared UNIQUE. Each one gets an automatic unique index built
    /// as part of the create.
    pub(crate) unique_on: Vec<usize>,
}

/// A DELETE with a WHERE clause.
#[derive(Debug)]
pub struct ResolvedDeleteQuery {
    /// The table rows are deleted from.
    pub(crate) table_name: String,
    /// Its root page.
    pub(crate) root_page: PageNo,
    /// The predicate, and the arena holding it. A DELETE with no predicate never
    /// reaches here, since it is resolved as a truncate instead.
    pub(crate) arena: Option<ExprArena>,

    pub(crate) where_clause: Option<usize>,
}

/// Emptying a whole table, which is faster than deleting its rows one at a time because the pages can be freed in bulk.
#[derive(Debug)]
pub struct ResolvedTruncateTableQuery {
    /// The table to empty, or to drop along with its pages.
    pub(crate) table_name: String,
    /// Its root page.
    pub(crate) root_page: u32,
}

use std::rc::Rc;
/// A CREATE INDEX.
#[derive(Debug)]
pub struct ResolvedCreateIndexQuery {
    /// The statement text, which goes into the catalog as the index DDL. An
    /// automatically created unique index has none, since the user never wrote it.
    pub(crate) query: Option<Rc<str>>,
    /// The root page of the table being indexed.
    pub(crate) relation_root_page: u32,
    /// The name of the table being indexed.
    pub(crate) relation_name: String,
    /// The name of the new index.
    pub(crate) index_name: String,
    /// The table column the index covers. Only one column is supported so far.
    pub(crate) column_index: usize, // todo: usize -> Vec::<usize>
    /// Whether the index rejects duplicate values.
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
/// An UPDATE, with its target columns resolved and the predicate bound.
#[derive(Debug)]
pub struct ResolvedUpdateQuery {
    /// The table being updated.
    pub(crate) table_name: String,
    /// Its root page.
    pub(crate) root_page: u32,
    /// The columns to change with the arena index of each new value, then the
    /// predicate and the arena both are held in.
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

/// An EXPLAIN, wrapped around the query it should describe.
#[derive(Debug)]
pub struct ResolvedExplainQuery {
    /// The resolved query whose plan tree will be printed.
    pub(crate) query: Box<ResolvedQuery>,
}

/// A statement that has been checked and is ready to be planned. Every variant
/// here is one shape of statement the engine knows how to run.
#[derive(Debug)]
pub enum ResolvedQuery {
    /// A SELECT.
    SelectQuery(ResolvedSelectQuery),
    /// A SELECT count(...).
    CountQuery(ResolvedCountQuery),
    /// An INSERT.
    InsertQuery(ResolvedInsertQuery),
    /// A CREATE TABLE.
    CreateTableQuery(ResolvedCreateTableQuery),
    /// A DROP TABLE.
    DropTblQuery(ResolvedDropTableQuery),
    /// A DROP INDEX.
    DropIndexQuery(ResolvedDropIndexQuery),
    /// A CREATE INDEX.
    CreateIndexQuery(ResolvedCreateIndexQuery),
    /// An UPDATE.
    UpdateQuery(ResolvedUpdateQuery),
    /// A DELETE with a WHERE clause.
    DeleteQuery(ResolvedDeleteQuery),
    /// Emptying or dropping a whole table.
    TruncateTable(ResolvedTruncateTableQuery),
    /// BEGIN.
    BeginTransactionQuery,
    /// COMMIT.
    CommitTransactionQuery,
    /// ROLLBACK.
    RollbackTransactionQuery,
    /// EXPLAIN.
    ExplainQuery(ResolvedExplainQuery),
}

impl<'a> Analyze<'a> {
    pub fn new(master: &'a Master) -> Self {
        Self { master }
    }

    /// Resolve one parsed statement.
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

    /// The table with this name, or the master table when that is the name asked
    /// for, since the master table is described in code rather than read from the
    /// catalog.
    pub(crate) fn get_table(&'a self, table_name: &str) -> Result<&'a Table, InkError> {
        if table_name.eq_ignore_ascii_case("master") {
            return Ok(&MASTER);
        }
        self.master
            .table(table_name)
            .ok_or_else(|| InkError::TableNotFound(table_name.to_string()))
    }
    /// The same lookup, for the statements that are not allowed to touch the
    /// master table at all.
    pub(crate) fn get_non_master_table(&self, table_name: &str) -> Result<&'a Table, InkError> {
        if table_name.eq_ignore_ascii_case("master") {
            return Err(InkError::MasterTableError);
        }
        self.master
            .table(table_name)
            .ok_or_else(|| InkError::TableNotFound(table_name.to_string()))
    }
}

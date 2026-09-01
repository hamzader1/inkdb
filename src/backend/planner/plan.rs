use super::super::executor::{project::Project, tablescan::TableScan};
use super::arena::Arena;
use crate::backend::analyze::{ResolvedInsertQuery, ResolvedQuery, ResolvedSelectQuery};
use crate::backend::executor::Row;
use crate::backend::executor::create::CreateTable;
use crate::backend::executor::eval::Eval;
use crate::backend::executor::filter::Filter;
use crate::backend::executor::insert::Insert;
use crate::backend::executor::limit::Limit;
use crate::backend::executor::transaction::{
    BeginTransaction, CommitTransaction, RollBackTransaction,
};
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;

#[derive(Debug)]
pub enum Plan<F: SqliteFile> {
    TableScan(TableScan<F>),
    Filter(Filter<F>),
    Limit(Limit<F>),
    Project(Project<F>),
    Insert(Insert<'static, F>),
    CreateTable(CreateTable),
    BeginTransaction(BeginTransaction),
    CommitTransaction(CommitTransaction),
    RollbackTransaction(RollBackTransaction),
}
pub enum PlanContext<F: SqliteFile> {
    Logical(Plan<F>),
    Resolved(Arena<F>),
}
impl<F: SqliteFile> PlanContext<F> {
    pub fn next(&mut self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        match self {
            Self::Logical(p) => p.next(pager, None),
            Self::Resolved(a) => a.next(pager),
        }
    }
}
impl<F: SqliteFile> Plan<F> {
    pub fn create_plan(
        resolved_query: ResolvedQuery,
        pager: &mut Pager<F>,
    ) -> Result<PlanContext<F>, SqliteError> {
        match resolved_query {
            ResolvedQuery::SelectQuery(stmt) => Ok(PlanContext::Resolved(
                Self::initialize_select_plan(stmt, pager)?,
            )),
            ResolvedQuery::InsertQuery(stmt) => {
                Ok(PlanContext::Resolved(Self::initialize_insert_plan(stmt)?))
            }
            ResolvedQuery::CreateTableQuery(stmt) => Ok(PlanContext::Logical(Plan::CreateTable(
                CreateTable::new(stmt),
            ))),
            ResolvedQuery::BeginTransactionQuery => Ok(PlanContext::Logical(
                Plan::BeginTransaction(BeginTransaction),
            )),
            ResolvedQuery::CommitTransactionQuery => Ok(PlanContext::Logical(
                Plan::CommitTransaction(CommitTransaction),
            )),
            ResolvedQuery::RollbackTransactionQuery => Ok(PlanContext::Logical(
                Plan::RollbackTransaction(RollBackTransaction),
            )),
            _ => todo!(),
        }
    }

    pub fn initialize_select_plan(
        resolved_query: ResolvedSelectQuery,
        pager: &mut Pager<F>,
    ) -> Result<Arena<F>, SqliteError> {
        let mut child = Self::TableScan(TableScan::new(resolved_query.root_page, pager)?);
        if let Some(predict) = resolved_query.where_clause {
            child = Self::Filter(Filter::new(Box::new(child), predict));
        }
        if let Some(limit) = resolved_query.limit {
            let limit = Eval::eval(&resolved_query.arena, limit)?.get_int()? as usize;
            child = Self::Limit(Limit::new(Box::new(child), limit));
        }
        let parent = Self::Project(Project::new(
            Box::new(child),
            resolved_query.columns.clone(),
        ));

        Ok(Arena::new(parent, Some(resolved_query.arena)))
    }

    pub fn initialize_insert_plan(
        resolved_query: ResolvedInsertQuery,
    ) -> Result<Arena<F>, SqliteError> {
        let plan = Plan::Insert(Insert::new(
            resolved_query.root,
            resolved_query.values,
            resolved_query.entry_hint,
        ));
        Ok(Arena {
            parent: plan,
            arena: None,
        })
    }
}

impl<F: SqliteFile> Plan<F> {
    pub fn next(
        &mut self,
        pager: &mut Pager<F>,
        arena: Option<&ExprArena>,
    ) -> Result<Option<Row>, SqliteError> {
        match self {
            Self::TableScan(t) => t.next(pager),
            Self::Filter(f) => f.next(pager, arena.unwrap()),
            Self::Limit(l) => l.next(pager, arena.unwrap()),
            Self::Project(p) => p.next(pager, arena.unwrap()),
            Self::Insert(i) => i.next(pager),
            Self::CreateTable(c) => c.next(pager),
            Self::BeginTransaction(bt) => bt.next(pager),
            Self::CommitTransaction(ct) => ct.next(pager),
            Self::RollbackTransaction(rbt) => rbt.next(pager),
            _ => todo!(),
        }
    }
}

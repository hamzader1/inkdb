use super::super::executor::{project::Project, tablescan::TableScan};
use super::prepared_plan::PreparedPlan;
use crate::backend::analyze::{
    ResolvedCreateIndexQuery, ResolvedDeleteQuery, ResolvedInsertQuery, ResolvedQuery,
    ResolvedSelectQuery,
};
use crate::backend::executor::Row;
use crate::backend::executor::create::{CreateIndex, CreateTable};
use crate::backend::executor::delete::Delete;
use crate::backend::executor::eval::Eval;
use crate::backend::executor::filter::Filter;
use crate::backend::executor::index::{BuildIndex, IndexExactMatch};
use crate::backend::executor::insert::Insert;
use crate::backend::executor::limit::Limit;
use crate::backend::executor::transaction::{
    BeginTransaction, CommitTransaction, RollBackTransaction,
};
use crate::backend::executor::truncate::TruncateTable;
use crate::backend::optimazer::Optimazer;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::disk::DiskFile;
use crate::vfs::file::SqliteFile;
use crate::{SqliteMaster, SqliteResult};

#[derive(Debug)]
pub enum Plan<F: SqliteFile> {
    TableScan(TableScan<F>),
    Filter(Filter<F>),
    Limit(Limit<F>),
    Project(Project<F>),
    Insert(Insert<'static, F>),
    Delete(Delete<F>),
    CreateTable(CreateTable),
    CreateIndex(CreateIndex<F>),
    BuildIndex(BuildIndex<F>),
    IndexExactMatch(IndexExactMatch<F>),
    TruncateTable(TruncateTable),
    BeginTransaction(BeginTransaction),
    CommitTransaction(CommitTransaction),
    RollbackTransaction(RollBackTransaction),
    Terminate(Terminate<F>),
}

impl<F: SqliteFile> Plan<F> {
    pub fn is_filter(&self) -> bool {
        matches!(*self, Plan::Filter(_))
    }
    /// Mutable child subtree for optimizer traversal (`while let Some(child)
    /// = plan.child_mut()`). Leaves and sinks return `None`.
    pub fn child_mut(&mut self) -> Option<&mut Plan<F>> {
        match self {
            Plan::Filter(f) => Some(f.child_mut()),
            Plan::Limit(l) => Some(l.child_mut()),
            Plan::Project(p) => Some(&mut p.child),
            Plan::Delete(d) => Some(d.child_mut()),
            _ => None,
        }
    }
}

pub enum PlanContext<F: SqliteFile> {
    Logical(Plan<F>),
    Resolved(PreparedPlan<F>),
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
        sqlite_master: &SqliteMaster,
    ) -> Result<PlanContext<F>, SqliteError> {
        match resolved_query {
            ResolvedQuery::SelectQuery(stmt) => Ok(PlanContext::Resolved(Self::init_select_plan(
                stmt,
                pager,
                sqlite_master,
            )?)),
            ResolvedQuery::InsertQuery(stmt) => {
                Ok(PlanContext::Resolved(Self::init_insert_plan(stmt)?))
            }
            ResolvedQuery::DeleteQuery(stmt) => {
                Ok(PlanContext::Resolved(Self::init_delete_plan(stmt, pager)?))
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
            ResolvedQuery::TruncateTable(stmt) => Ok(PlanContext::Resolved(PreparedPlan::new(
                Plan::TruncateTable(TruncateTable::new(stmt.root_page)),
                None,
            ))),
            ResolvedQuery::CreateIndexQuery(stmt) => Ok(PlanContext::Resolved(
                Self::init_create_index_plan(stmt, pager)?,
            )),
            _ => todo!(),
        }
    }

    pub fn init_select_plan(
        resolved_query: ResolvedSelectQuery,
        pager: &mut Pager<F>,
        sqlite_master: &SqliteMaster,
    ) -> Result<PreparedPlan<F>, SqliteError> {
        let mut child = Self::TableScan(TableScan::new(resolved_query.root_page, pager)?);
        if let Some(predict) = resolved_query.where_clause {
            child = Self::Filter(Filter::new(Box::new(child), predict));
        }
        if let Some(limit) = resolved_query.limit {
            let limit = Eval::eval(&resolved_query.arena, limit, None)?.get_int()? as usize;
            child = Self::Limit(Limit::new(Box::new(child), limit));
        }
        let mut parent = Self::Project(Project::new(
            Box::new(child),
            resolved_query.columns.clone(),
        ));
        Optimazer::optimaze_select(
            &mut parent,
            pager,
            sqlite_master,
            &resolved_query.table_name,
            resolved_query.root_page,
            &resolved_query.arena,
        )?;
        Ok(PreparedPlan::new(parent, Some(resolved_query.arena)))
    }

    pub fn init_insert_plan(
        resolved_query: ResolvedInsertQuery,
    ) -> Result<PreparedPlan<F>, SqliteError> {
        // Insert Table Row and return it to Insert Index
        let mut plan = Plan::Insert(Insert::new(
            resolved_query.root_page,
            resolved_query.values,
            resolved_query.entry_hint,
        ));
        // dbg!(&resolved_query.indexes);
        if let Some(indexes) = resolved_query.indexes {
            for index in indexes {
                println!("INDEX ON {}", index.col_idx);
                plan = Plan::BuildIndex(BuildIndex::new(
                    index.index_root_page,
                    index.col_idx,
                    index.is_unique,
                    Box::new(plan),
                ));
            }
        }

        let plan = Plan::Terminate(Terminate {
            child: Box::new(plan),
        });
        Ok(PreparedPlan {
            parent: plan,
            arena: None,
        })
    }

    pub fn init_delete_plan(
        resolved_query: ResolvedDeleteQuery,
        pager: &mut Pager<F>,
    ) -> SqliteResult<PreparedPlan<F>> {
        let mut child = Self::TableScan(TableScan::new(resolved_query.root_page, pager)?);
        if let Some(predict) = resolved_query.where_clause {
            child = Self::Filter(Filter::new(Box::new(child), predict));
        }
        let parent = Self::Delete(Delete::new(Box::new(child), resolved_query.root_page));
        Ok(PreparedPlan::new(parent, resolved_query.arena))
    }

    pub fn init_create_index_plan(
        resolved_query: ResolvedCreateIndexQuery,
        pager: &mut Pager<F>,
    ) -> SqliteResult<PreparedPlan<F>> {
        let child = Self::TableScan(TableScan::new(resolved_query.relation_root_page, pager)?);
        let parent = Self::CreateIndex(CreateIndex::new(Box::new(child), resolved_query, pager)?);
        Ok(PreparedPlan::new(parent, None))
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
            Self::Delete(d) => d.next(pager, arena),
            Self::CreateTable(c) => c.next(pager),
            Self::BeginTransaction(bt) => bt.next(pager),
            Self::CommitTransaction(ct) => ct.next(pager),
            Self::RollbackTransaction(rbt) => rbt.next(pager),
            Self::TruncateTable(tb) => tb.next(pager),
            Self::IndexExactMatch(iem) => iem.next(pager, arena.unwrap()),
            Self::CreateIndex(ci) => ci.next(pager),
            Self::BuildIndex(bi) => bi.next(pager),
            Self::Terminate(t) => t.next(pager),
            _ => todo!(),
        }
    }
}

// he job of this is only to not yeild any row
#[derive(Debug)]
pub struct Terminate<F: SqliteFile> {
    child: Box<Plan<F>>,
}

impl<F: SqliteFile> Terminate<F> {
    pub fn new(child: Box<Plan<F>>) -> Self {
        Self { child }
    }

    pub fn next(&mut self, pager: &mut Pager<F>) -> SqliteResult<Option<Row>> {
        while self.child.next(pager, None)?.is_some() {}
        Ok(None)
    }
}

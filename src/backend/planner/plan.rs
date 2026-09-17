use self::Plan::Halt;

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
use crate::backend::executor::index::{
    IndexDelete, IndexExactMatch, IndexInsert, IndexRangeScan, PrepareIndex,
};
use crate::backend::executor::insert::Insert;
use crate::backend::executor::limit::Limit;
use crate::backend::executor::prepare::PrepareRow;
use crate::backend::executor::scan_guard::{CustomScanGuard, SafeScan, ScanGuard, UnsafeScan};
use crate::backend::executor::transaction::{
    BeginTransaction, CommitTransaction, RollBackTransaction,
};
use crate::backend::executor::truncate::TruncateTable;
use crate::backend::optimizer::Optimizer;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;
use crate::{SqliteMaster, SqliteResult};

#[derive(Debug)]
pub enum Plan<F: SqliteFile> {
    TableScan(TableScan<F>),
    Filter(Filter<F>),
    Limit(Limit<F>),
    Project(Project<F>),
    Insert(Insert<'static, F>),
    PrepareRow(PrepareRow<F>),
    Delete(Delete<F>),
    CreateTable(CreateTable),
    CreateIndex(CreateIndex<F>),
    PrepareIndex(PrepareIndex<F>),
    IndexExactMatch(IndexExactMatch<F>),
    IndexRangeScan(IndexRangeScan<F>),
    TruncateTable(TruncateTable),
    BeginTransaction(BeginTransaction),
    CommitTransaction(CommitTransaction),
    RollbackTransaction(RollBackTransaction),
    Explain(Explain<F>),
    Terminate(Terminate<F>),
    Halt,
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
            Plan::PrepareIndex(pi) => Some(pi.child_mut()),
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
            ResolvedQuery::DeleteQuery(stmt) => Ok(PlanContext::Resolved(Self::init_delete_plan(
                stmt,
                pager,
                sqlite_master,
            )?)),
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
                Plan::TruncateTable(TruncateTable::new(stmt.root_page, stmt.indexes)),
                None,
            ))),
            ResolvedQuery::CreateIndexQuery(stmt) => Ok(PlanContext::Resolved(
                Self::init_create_index_plan(stmt, pager)?,
            )),
            ResolvedQuery::ExplainQuery(stmt) => {
                let plan = Self::create_plan(*stmt.query, pager, sqlite_master)?;
                match plan {
                    PlanContext::Logical(p) => Ok(PlanContext::Logical(Self::Explain(Explain {
                        child: Box::new(p),
                    }))),
                    PlanContext::Resolved(r) => Ok(PlanContext::Logical(Self::Explain(Explain {
                        child: Box::new(r.parent),
                    }))),
                }
            }
            _ => todo!(),
        }
    }

    pub fn init_select_plan(
        resolved_query: ResolvedSelectQuery,
        pager: &mut Pager<F>,
        sqlite_master: &SqliteMaster,
    ) -> Result<PreparedPlan<F>, SqliteError> {
        let mut child = Self::TableScan(TableScan::new(
            resolved_query.root_page,
            pager,
            Box::new(SafeScan),
        )?);
        if let Some(predict) = resolved_query.where_clause {
            child = Self::Filter(Filter::new(Box::new(child), predict));
            Optimizer::new(
                &mut child,
                pager,
                sqlite_master,
                &resolved_query.table_name,
                resolved_query.root_page,
                &resolved_query.arena,
                CustomScanGuard::new(Some(|| -> Box<dyn ScanGuard<F>> { Box::new(SafeScan) })),
            )
            .optimize()?;
        }
        if let Some(limit) = resolved_query.limit {
            let limit = Eval::eval(&resolved_query.arena, limit, None)?.cast_int()? as usize;
            child = Self::Limit(Limit::new(Box::new(child), limit));
        }
        let mut parent = Self::Project(Project::new(
            Box::new(child),
            resolved_query.columns.clone(),
        ));

        Ok(PreparedPlan::new(parent, Some(resolved_query.arena)))
    }

    pub fn init_insert_plan(
        resolved_query: ResolvedInsertQuery,
    ) -> Result<PreparedPlan<F>, SqliteError> {
        // Rows flow upward: PrepareRow yields table rows, each PrepareIndex
        // writes one index entry per row and passes it along
        let mut plan = Plan::PrepareRow(PrepareRow::new(
            None,
            resolved_query.root_page,
            resolved_query.values,
            None, // table constraints hook
        ));
        if let Some(indexes) = resolved_query.indexes {
            for index in indexes {
                let prepare = PrepareIndex::new(
                    index.index_root_page,
                    index.col_idx,
                    Box::new(IndexInsert {
                        is_unique: index.is_unique,
                    }),
                    Box::new(plan),
                );
                plan = Plan::PrepareIndex(prepare);
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
        sqlite_master: &SqliteMaster,
    ) -> SqliteResult<PreparedPlan<F>> {
        let mut parent = Self::TableScan(TableScan::new(
            resolved_query.root_page,
            pager,
            Box::new(UnsafeScan),
        )?);
        if let Some(predict) = resolved_query.where_clause {
            parent = Self::Filter(Filter::new(Box::new(parent), predict));

            Optimizer::new(
                &mut parent,
                pager,
                sqlite_master,
                &resolved_query.table_name,
                resolved_query.root_page,
                resolved_query.arena.as_ref().unwrap(),
                CustomScanGuard::new(Some(|| -> Box<dyn ScanGuard<F>> { Box::new(UnsafeScan) })),
            )
            .optimize()?;
        }

        if let Some(indexes) = resolved_query.indexes {
            for index in indexes {
                let prepare = PrepareIndex::new(
                    index.index_root_page,
                    index.col_idx,
                    Box::new(IndexDelete),
                    Box::new(parent),
                );
                parent = Plan::PrepareIndex(prepare);
            }
        }
        parent = Self::Delete(Delete::new(Box::new(parent), resolved_query.root_page));
        Ok(PreparedPlan::new(parent, resolved_query.arena))
    }

    pub fn init_create_index_plan(
        resolved_query: ResolvedCreateIndexQuery,
        pager: &mut Pager<F>,
    ) -> SqliteResult<PreparedPlan<F>> {
        let child = Self::TableScan(TableScan::new(
            resolved_query.relation_root_page,
            pager,
            Box::new(SafeScan),
        )?);
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
            Self::IndexRangeScan(irc) => irc.next(pager),
            Self::CreateIndex(ci) => ci.next(pager),
            Self::Terminate(t) => t.next(pager),
            Self::PrepareIndex(pi) => pi.next(pager, arena),
            Self::PrepareRow(pr) => pr.next(pager),
            Self::Explain(e) => e.next(arena),
            Halt => Ok(None),
            _ => unreachable!(),
        }
    }
}

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

impl<F: SqliteFile> Plan<F> {
    pub fn explain_plan(&self, arena: Option<&ExprArena>) -> String {
        match self {
            Self::TableScan(tb) => {
                format!(
                    "TableScan [root_page: {}, scan_plan: {}]",
                    tb.cursor.root,
                    tb.guard.scan_type()
                )
            }
            Self::Filter(f) => match arena {
                Some(a) => format!("Filter [{:?}]", a.nodes[f.predicate()]),
                None => format!("Filter [pred: {}]", f.predicate()),
            },
            Self::Limit(l) => {
                format!("LIMIT [limit: {}]", l.limit)
            }
            Self::Insert(i) => format!(
                "INSERT [root_page: {}, key: {}, data..]",
                i.root_page, i.key
            ),
            Self::PrepareRow(pr) => format!(
                "PrepareRow [root_page: {}, rows: {:#?}",
                pr.root_page, pr.rows
            ),
            Self::Project(p) => format!("Project [columns: {:?}]", p.columns()),
            Self::Delete(d) => format!("Delete [root_page: {}]", d.root_page()),
            Self::CreateTable(c) => format!("CreateTable [name: {}]", c.table_name()),
            Self::CreateIndex(c) => format!(
                "CreateIndex [index_root: {}, col: {}]",
                c.index_root_page(),
                c.col_idx()
            ),
            Self::PrepareIndex(p) => format!(
                "PrepareIndex [index_root: {}, col: {}, action: {}]",
                p.index_root_page(),
                p.col_idx(),
                p.action_name()
            ),
            Self::IndexExactMatch(i) => format!(
                "IndexExactMatch [index_root: {}, table_root: {}, target: {}]",
                i.index_root_page(),
                i.relation_root_page(),
                i.target()
            ),
            Self::TruncateTable(t) => format!(
                "TruncateTable [root_page: {}, indexes: {:?}]",
                t.root_page(),
                t.indexes()
            ),
            Self::BeginTransaction(_) => "BeginTransaction".into(),
            Self::CommitTransaction(_) => "CommitTransaction".into(),
            Self::RollbackTransaction(_) => "RollbackTransaction".into(),
            Self::Explain(_) => "Explain".into(),
            Self::Terminate(_) => "Terminate".into(),
            Self::IndexRangeScan(i) => format!(
                "IndexRangeScan [index_root: {}, table_root: {}, target: {:?}]",
                i.index_root_page(),
                i.relation_root_page(),
                i.range()
            ),
            _ => unreachable!(),
        }
    }
}

#[derive(Debug)]
pub struct Explain<F: SqliteFile> {
    child: Box<Plan<F>>,
}

impl<F: SqliteFile> Explain<F> {
    fn new(child: Box<Plan<F>>) -> Self {
        Self { child }
    }

    fn next(&mut self, arena: Option<&ExprArena>) -> SqliteResult<Option<Row>> {
        let mut plan = &mut *self.child;
        println!("{}", plan.explain_plan(arena));
        while let Some(child) = plan.child_mut() {
            println!("{}", child.explain_plan(arena));
            plan = child;
        }
        Ok(None)
    }
}

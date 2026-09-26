use self::Plan::Halt;

use super::super::executor::{project::Project, tablescan::TableScan};
use super::prepared_plan::PreparedPlan;
use crate::backend::analyze::{
    ResolvedCreateIndexQuery, ResolvedDeleteQuery, ResolvedInsertQuery, ResolvedQuery,
    ResolvedSelectQuery,
};
use crate::backend::executor::Row;
use crate::backend::executor::context::ExecCtx;
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
use crate::backend::executor::scan_guard::ScanMode;
use crate::backend::executor::transaction::{
    BeginTransaction, CommitTransaction, RollBackTransaction,
};
use crate::backend::executor::truncate::TruncateTable;
use crate::backend::optimizer::optimize_index_scan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;
use crate::{SqliteMaster, SqliteResult};

#[derive(Debug)]
pub enum Plan<V: Vfs> {
    TableScan(TableScan<V>),
    Filter(Filter<V>),
    Limit(Limit<V>),
    Project(Project<V>),
    Insert(Insert<'static, V>),
    PrepareRow(PrepareRow<V>),
    Delete(Delete<V>),
    CreateTable(CreateTable),
    CreateIndex(CreateIndex<V>),
    PrepareIndex(PrepareIndex<V>),
    IndexExactMatch(IndexExactMatch<V>),
    IndexRangeScan(IndexRangeScan<V>),
    TruncateTable(TruncateTable),
    BeginTransaction(BeginTransaction),
    CommitTransaction(CommitTransaction),
    RollbackTransaction(RollBackTransaction),
    Explain(Explain<V>),
    Terminate(Terminate<V>),
    Halt,
}

impl<V: Vfs> Plan<V> {
    pub fn children(&self) -> Vec<&Plan<V>> {
        match self {
            Plan::Filter(f) => vec![f.child()],
            Plan::Limit(l) => vec![l.child()],
            Plan::Project(p) => vec![p.child()],
            Plan::Delete(d) => vec![d.child()],
            Plan::PrepareIndex(pi) => vec![pi.child()],
            Plan::CreateIndex(ci) => vec![ci.child()],
            Plan::Terminate(t) => vec![t.child()],
            Plan::Explain(e) => vec![e.child()],
            _ => Vec::new(),
        }
    }
}

pub struct PlanTree<'a, V: Vfs> {
    plan: &'a Plan<V>,
    arena: &'a ExprArena,
}

impl<'a, V: Vfs> PlanTree<'a, V> {
    pub fn new(plan: &'a Plan<V>, arena: &'a ExprArena) -> Self {
        Self { plan, arena }
    }

    fn write_tree(&self, f: &mut std::fmt::Formatter<'_>, depth: usize) -> std::fmt::Result {
        writeln!(
            f,
            "{}{}",
            "    ".repeat(depth),
            self.plan.node_label(self.arena)
        )?;
        for child in self.plan.children() {
            Self::new(child, self.arena).write_tree(f, depth + 1)?;
        }
        Ok(())
    }
}

impl<'a, V: Vfs> std::fmt::Display for PlanTree<'a, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.write_tree(f, 0)
    }
}

impl<V: Vfs> Plan<V> {
    #[allow(clippy::only_used_in_recursion)]
    pub fn create_plan(
        resolved_query: ResolvedQuery,
        pager: &mut Pager<V>,
        sqlite_master: &SqliteMaster,
    ) -> Result<PreparedPlan<V>, SqliteError> {
        match resolved_query {
            ResolvedQuery::SelectQuery(stmt) => Self::init_select_plan(stmt, sqlite_master),
            ResolvedQuery::InsertQuery(stmt) => Self::init_insert_plan(stmt, sqlite_master),
            ResolvedQuery::DeleteQuery(stmt) => Self::init_delete_plan(stmt, sqlite_master),
            ResolvedQuery::CreateTableQuery(stmt) => Ok(PreparedPlan::new(
                Plan::CreateTable(CreateTable::new(stmt)),
                ExprArena::new(),
            )),
            ResolvedQuery::BeginTransactionQuery => Ok(PreparedPlan::new(
                Plan::BeginTransaction(BeginTransaction),
                ExprArena::new(),
            )),
            ResolvedQuery::CommitTransactionQuery => Ok(PreparedPlan::new(
                Plan::CommitTransaction(CommitTransaction),
                ExprArena::new(),
            )),
            ResolvedQuery::RollbackTransactionQuery => Ok(PreparedPlan::new(
                Plan::RollbackTransaction(RollBackTransaction),
                ExprArena::new(),
            )),
            ResolvedQuery::TruncateTable(stmt) => {
                let indexes = index_roots(sqlite_master, &stmt.table_name)?;
                Ok(PreparedPlan::new(
                    Plan::TruncateTable(TruncateTable::new(stmt.root_page, indexes)),
                    ExprArena::new(),
                ))
            }
            ResolvedQuery::CreateIndexQuery(stmt) => Self::init_create_index_plan(stmt),
            ResolvedQuery::ExplainQuery(stmt) => {
                let inner = Self::create_plan(*stmt.query, pager, sqlite_master)?;
                Ok(PreparedPlan::new(
                    Plan::Explain(Explain::new(Box::new(inner.parent))),
                    inner.arena,
                ))
            }
        }
    }

    fn init_select_plan(
        resolved_query: ResolvedSelectQuery,
        sqlite_master: &SqliteMaster,
    ) -> Result<PreparedPlan<V>, SqliteError> {
        let mode = ScanMode::Stable;
        let mut child = Self::TableScan(TableScan::new(resolved_query.root_page, mode)?);
        if let Some(predicate) = resolved_query.where_clause {
            child = Self::Filter(Filter::new(Box::new(child), predicate));
            optimize_index_scan(
                &mut child,
                sqlite_master,
                &resolved_query.table_name,
                &resolved_query.arena,
                mode,
            )?;
            if let Plan::Filter(f) = &mut child
                && let Plan::TableScan(scan) = f.child_mut()
            {
                scan.set_predicate(predicate);
            }
        }
        if let Some(limit) = resolved_query.limit {
            let limit = Eval::eval(&resolved_query.arena, limit, None)?.cast_int()? as usize;
            child = Self::Limit(Limit::new(Box::new(child), limit));
        }
        let parent = Self::Project(Project::new(
            Box::new(child),
            resolved_query.columns.clone(),
        ));

        Ok(PreparedPlan::new(parent, resolved_query.arena))
    }

    fn init_insert_plan(
        resolved_query: ResolvedInsertQuery,
        sqlite_master: &SqliteMaster,
    ) -> Result<PreparedPlan<V>, SqliteError> {
        let mut plan = Plan::PrepareRow(PrepareRow::new(
            None,
            resolved_query.root_page,
            resolved_query.values,
            None,
        ));
        for index in sqlite_master.indexes_on(&resolved_query.table_name)? {
            let prepare = PrepareIndex::new(
                index,
                Box::new(IndexInsert {
                    is_unique: index.is_unique,
                }),
                Box::new(plan),
            );
            plan = Plan::PrepareIndex(prepare);
        }

        let plan = Plan::Terminate(Terminate::new(Box::new(plan)));
        Ok(PreparedPlan::new(plan, ExprArena::new()))
    }

    fn init_delete_plan(
        mut resolved_query: ResolvedDeleteQuery,
        sqlite_master: &SqliteMaster,
    ) -> SqliteResult<PreparedPlan<V>> {
        let mode = ScanMode::Volatile;
        let arena = resolved_query.arena.take().unwrap_or_default();
        let mut parent = Self::TableScan(TableScan::new(resolved_query.root_page, mode)?);
        if let Some(predicate) = resolved_query.where_clause {
            parent = Self::Filter(Filter::new(Box::new(parent), predicate));
            optimize_index_scan(
                &mut parent,
                sqlite_master,
                &resolved_query.table_name,
                &arena,
                mode,
            )?;
        }

        for index in sqlite_master.indexes_on(&resolved_query.table_name)? {
            let prepare = PrepareIndex::new(index, Box::new(IndexDelete), Box::new(parent));
            parent = Plan::PrepareIndex(prepare);
        }
        parent = Self::Delete(Delete::new(Box::new(parent), resolved_query.root_page));
        Ok(PreparedPlan::new(parent, arena))
    }

    fn init_create_index_plan(
        resolved_query: ResolvedCreateIndexQuery,
    ) -> SqliteResult<PreparedPlan<V>> {
        let child = Self::TableScan(TableScan::new(
            resolved_query.relation_root_page,
            ScanMode::Stable,
        )?);
        let parent = Self::CreateIndex(CreateIndex::new(Box::new(child), resolved_query)?);
        Ok(PreparedPlan::new(parent, ExprArena::new()))
    }
}

impl<V: Vfs> Plan<V> {
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        match self {
            Self::TableScan(t) => t.next(ctx),
            Self::Filter(f) => f.next(ctx),
            Self::Limit(l) => l.next(ctx),
            Self::Project(p) => p.next(ctx),
            Self::Insert(i) => i.next(ctx),
            Self::Delete(d) => d.next(ctx),
            Self::CreateTable(c) => c.next(ctx),
            Self::BeginTransaction(bt) => bt.next(ctx),
            Self::CommitTransaction(ct) => ct.next(ctx),
            Self::RollbackTransaction(rbt) => rbt.next(ctx),
            Self::TruncateTable(tb) => tb.next(ctx),
            Self::IndexExactMatch(iem) => iem.next(ctx),
            Self::IndexRangeScan(irc) => irc.next(ctx),
            Self::CreateIndex(ci) => ci.next(ctx),
            Self::Terminate(t) => t.next(ctx),
            Self::PrepareIndex(pi) => pi.next(ctx),
            Self::PrepareRow(pr) => pr.next(ctx),
            Self::Explain(e) => e.next(ctx),
            Halt => Ok(None),
        }
    }

    pub fn node_label(&self, arena: &ExprArena) -> String {
        match self {
            Self::TableScan(tb) => format!(
                "TableScan [root_page: {}, scan: {}]",
                tb.cursor.root,
                tb.guard.scan_type()
            ),
            Self::Filter(f) => match arena.nodes.get(f.predicate()) {
                Some(expr) => format!("Filter [{expr:?}]"),
                None => format!("Filter [pred: {}]", f.predicate()),
            },
            Self::Limit(l) => format!("Limit [limit: {}]", l.limit),
            Self::Insert(i) => format!(
                "Insert [root_page: {}, key: {}, data: {} bytes]",
                i.root_page,
                i.key,
                i.data.len()
            ),
            Self::PrepareRow(pr) => format!(
                "PrepareRow [root_page: {}, rows: {:?}]",
                pr.root_page, pr.rows
            ),
            Self::Project(p) => format!("Project [columns: {:?}]", p.columns()),
            Self::Delete(d) => format!("Delete [root_page: {}]", d.root_page()),
            Self::CreateTable(c) => format!("CreateTable [name: {}]", c.table_name()),
            Self::CreateIndex(c) => format!("CreateIndex [indexed_column: {}]", c.col_idx()),
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
            Self::IndexRangeScan(i) => format!(
                "IndexRangeScan [index_root: {}, table_root: {}, target: {:?}]",
                i.index_root_page(),
                i.relation_root_page(),
                i.range()
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
            Halt => "Halt".into(),
        }
    }
}

fn index_roots(sqlite_master: &SqliteMaster, table_name: &str) -> SqliteResult<Vec<u32>> {
    Ok(sqlite_master
        .indexes_on(table_name)?
        .into_iter()
        .map(|index| index.index_root_page)
        .collect())
}

#[derive(Debug)]
pub struct Terminate<V: Vfs> {
    child: Box<Plan<V>>,
}

impl<V: Vfs> Terminate<V> {
    pub fn new(child: Box<Plan<V>>) -> Self {
        Self { child }
    }

    pub fn child(&self) -> &Plan<V> {
        &self.child
    }

    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        while self.child.next(ctx)?.is_some() {}
        Ok(None)
    }
}

#[derive(Debug)]
pub struct Explain<V: Vfs> {
    child: Box<Plan<V>>,
}

impl<V: Vfs> Explain<V> {
    fn new(child: Box<Plan<V>>) -> Self {
        Self { child }
    }

    pub fn child(&self) -> &Plan<V> {
        &self.child
    }

    fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        println!("{}", PlanTree::new(&self.child, ctx.arena));
        Ok(None)
    }
}

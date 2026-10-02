use self::Plan::Halt;

use super::super::executor::{project::Project, tablescan::TableScan};
use super::prepared_plan::PreparedPlan;
use crate::backend::analyzer::{
    ResolvedCountQuery, ResolvedCreateIndexQuery, ResolvedDeleteQuery, ResolvedInsertQuery,
    ResolvedQuery, ResolvedSelectQuery, ResolvedUpdateQuery,
};
use crate::backend::executor::Row;
use crate::backend::executor::aggregate::Count;
use crate::backend::executor::context::ExecCtx;
use crate::backend::executor::create::{CreateIndex, CreateTable};
use crate::backend::executor::delete::Delete;
use crate::backend::executor::eval::{Eval, render_expr};
use crate::backend::executor::filter::Filter;
use crate::backend::executor::index::{
    IndexDelete, IndexExactMatch, IndexInsert, IndexRangeScan, PrepareIndex,
};
use crate::backend::executor::insert::Insert;
use crate::backend::executor::limit::Limit;
use crate::backend::executor::materialized::MaterializedResult;
use crate::backend::executor::prepare::{PrepareInsert, PrepareRow};
use crate::backend::executor::scan_guard::ScanMode;
use crate::backend::executor::sort::Sort;
use crate::backend::executor::transaction::{
    BeginTransaction, CommitTransaction, RollBackTransaction,
};
use crate::backend::executor::truncate::TruncateTable;
use crate::backend::executor::update::Update;
use crate::backend::optimizer::optimize_index_scan;
use crate::backend::planner::plan;
use crate::errors::InkError;
use crate::pager::pager::Pager;
use crate::schema::Table;
use crate::sql::ast::Expr;
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;
use crate::{InkResult, Master};

#[derive(Debug, Default)]
pub enum Plan<V: Vfs> {
    TableScan(TableScan<V>),
    Filter(Filter<V>),
    Count(Count<V>),
    Limit(Limit<V>),
    Project(Project<V>),
    Insert(Insert<'static, V>),
    Update(Update<V>),
    PrepareRow(PrepareRow<V>),
    Delete(Delete<V>),
    CreateTable(CreateTable),
    CreateIndex(CreateIndex<V>),
    PrepareIndex(PrepareIndex<V>),
    PrepareInsert(PrepareInsert<V>),
    IndexExactMatch(IndexExactMatch<V>),
    IndexRangeScan(IndexRangeScan<V>),
    TruncateTable(TruncateTable),
    BeginTransaction(BeginTransaction),
    Materialized(MaterializedResult<V>),
    CommitTransaction(CommitTransaction),
    RollbackTransaction(RollBackTransaction),
    Sort(Sort<V>),
    Explain(Explain<V>),
    Terminate(Terminate<V>),
    #[default]
    Halt,
}

impl<V: Vfs> Plan<V> {
    pub fn children(&self) -> Vec<&Plan<V>> {
        match self {
            Plan::Filter(f) => vec![f.child()],
            Plan::Count(c) => vec![c.child()],
            Plan::Limit(l) => vec![l.child()],
            Plan::Project(p) => vec![p.child()],
            Plan::Delete(d) => vec![d.child()],
            Plan::PrepareIndex(pi) => vec![pi.child()],
            Plan::CreateIndex(ci) => vec![ci.child()],
            Plan::Terminate(t) => vec![t.child()],
            Plan::Explain(e) => vec![e.child()],
            Plan::Sort(s) => vec![s.child()],
            Plan::PrepareRow(pr) => vec![pr.child()],
            Plan::Materialized(m) => vec![m.child()],
            Plan::Delete(d) => vec![d.child()],
            Plan::Update(u) => vec![u.child()],
            _ => Vec::new(),
        }
    }
}

pub struct PlanTree<'a, V: Vfs> {
    plan: &'a Plan<V>,
    arena: &'a ExprArena,
    table: Option<&'a Table>,
}

impl<'a, V: Vfs> PlanTree<'a, V> {
    pub fn new(plan: &'a Plan<V>, arena: &'a ExprArena, table: Option<&'a Table>) -> Self {
        Self { plan, arena, table }
    }

    fn write_tree(&self, f: &mut std::fmt::Formatter<'_>, depth: usize) -> std::fmt::Result {
        writeln!(
            f,
            "{}{}",
            "    ".repeat(depth),
            self.plan.node_label(self.arena, self.table)
        )?;
        for child in self.plan.children() {
            Self::new(child, self.arena, self.table).write_tree(f, depth + 1)?;
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
        master: &Master,
    ) -> Result<PreparedPlan<V>, InkError> {
        match resolved_query {
            ResolvedQuery::SelectQuery(stmt) => Self::init_select_plan(stmt, master),
            ResolvedQuery::CountQuery(stmt) => Self::init_count_plan(stmt, master),
            ResolvedQuery::InsertQuery(stmt) => Self::init_insert_plan(stmt, master),
            ResolvedQuery::DeleteQuery(stmt) => Self::init_delete_plan(stmt, master),
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
                let indexes = index_roots(master, &stmt.table_name)?;
                Ok(PreparedPlan::new(
                    Plan::TruncateTable(TruncateTable::new(stmt.root_page, indexes)),
                    ExprArena::new(),
                ))
            }
            ResolvedQuery::CreateIndexQuery(stmt) => Self::init_create_index_plan(stmt),
            ResolvedQuery::ExplainQuery(stmt) => {
                let inner = Self::create_plan(*stmt.query, pager, master)?;
                let mut prepared = PreparedPlan::new(
                    Plan::Explain(Explain::new(Box::new(inner.parent))),
                    inner.arena,
                );
                prepared.statement_table = inner.statement_table;
                Ok(prepared)
            }
            ResolvedQuery::UpdateQuery(stmt) => Self::init_update_plan(stmt, master),
            _ => todo!(),
        }
    }

    fn scan_with_predicate(
        root_page: u32,
        table_name: &str,
        mode: ScanMode,
        predicate: Option<usize>,
        master: &Master,
        arena: &ExprArena,
    ) -> InkResult<Plan<V>> {
        let mut scan = TableScan::new(root_page, mode, table_name.to_string())?;
        let Some(predicate) = predicate else {
            return Ok(Self::TableScan(scan));
        };
        scan.set_pushed_predicate(predicate);
        let mut plan = Self::Filter(Filter::new(Box::new(Self::TableScan(scan)), predicate));
        optimize_index_scan(&mut plan, master, table_name, arena, mode)?;
        match plan {
            Self::Filter(filter) if matches!(filter.child(), Self::TableScan(_)) => {
                Ok(*filter.into_child())
            }
            other => Ok(other),
        }
    }

    fn init_select_plan(
        resolved_query: ResolvedSelectQuery,
        master: &Master,
    ) -> Result<PreparedPlan<V>, InkError> {
        let mode = ScanMode::Safe;
        let mut child = Self::scan_with_predicate(
            resolved_query.root_page,
            &resolved_query.table_name,
            mode,
            resolved_query.where_clause,
            master,
            &resolved_query.arena,
        )?;
        if let Some(orderby) = resolved_query.orderby {
            child = Self::Sort(Sort::new(Box::new(child), orderby.index, orderby.desc));
        }
        if let Some(limit) = resolved_query.limit {
            let limit = Eval::eval(&resolved_query.arena, limit, None)?.cast_int()? as usize;
            child = Self::Limit(Limit::new(Box::new(child), limit));
        }
        /*
         * Optimization: skip the projection when it is an identity projection
         * (all relation columns are selected in their original order).
         * Relation R: (C0, C1, C2)
         * Query on R: (Ci..Ck+i where k <= i<= Rmax)
         */
        let columns = resolved_query.columns.clone();
        let identity = master.table(&resolved_query.table_name).is_some_and(|table| {
            columns.len() == table.get_cols_len()
                && columns.iter().enumerate().all(|(position, node)| {
                    matches!(resolved_query.arena.nodes[*node], Expr::ColumnRef(column) if column == position)
                })
        });
        let parent = if identity {
            child
        } else {
            Self::Project(Project::new(Box::new(child), columns))
        };

        Ok(PreparedPlan::new(parent, resolved_query.arena).with_table(&resolved_query.table_name))
    }

    fn init_count_plan(
        resolved_query: ResolvedCountQuery,
        master: &Master,
    ) -> InkResult<PreparedPlan<V>> {
        let mode = ScanMode::Safe;
        let mode = ScanMode::Safe;
        let mut child = Self::scan_with_predicate(
            resolved_query.root_page,
            &resolved_query.table_name,
            mode,
            resolved_query.where_clause,
            master,
            &resolved_query.arena,
        )?;
        let mut parent = Self::Count(Count::new(Box::new(child), resolved_query.arg));
        if let Some(limit_expr) = resolved_query.limit {
            let limit = Eval::eval(&resolved_query.arena, limit_expr, None)?.cast_int()? as usize;
            parent = Self::Limit(Limit::new(Box::new(parent), limit));
        }
        Ok(PreparedPlan::new(parent, resolved_query.arena).with_table(&resolved_query.table_name))
    }

    fn init_insert_plan(
        resolved_query: ResolvedInsertQuery,
        master: &Master,
    ) -> Result<PreparedPlan<V>, InkError> {
        let mut plan = Plan::PrepareInsert(PrepareInsert::new(resolved_query.values));
        plan = Self::init_insert_plan_inner(
            plan,
            master,
            resolved_query.table_name,
            resolved_query.root_page,
        )?;
        plan = Plan::Terminate(Terminate::new(Box::new(plan)));
        Ok(PreparedPlan::new(plan, ExprArena::new()))
    }

    fn init_insert_plan_inner(
        mut plan: Plan<V>,
        master: &Master,
        table_name: String,
        root_page: u32,
    ) -> InkResult<Plan<V>> {
        let indexes = master.indexes_on(&table_name)?;
        plan = Plan::PrepareRow(PrepareRow::<V>::new(
            Box::new(plan),
            root_page,
            table_name,
            None,
        ));
        for index in indexes {
            let prepare = PrepareIndex::new(
                index,
                Box::new(IndexInsert {
                    is_unique: index.is_unique,
                }),
                Box::new(plan),
            );
            plan = Plan::PrepareIndex(prepare);
        }
        Ok(plan)
    }

    fn init_delete_plan(
        mut resolved_query: ResolvedDeleteQuery,
        master: &Master,
    ) -> InkResult<PreparedPlan<V>> {
        let mode = ScanMode::Unsafe;
        let arena = resolved_query.arena.take().unwrap_or_default();
        let mut parent = Self::scan_with_predicate(
            resolved_query.root_page,
            &resolved_query.table_name,
            mode,
            resolved_query.where_clause,
            master,
            &arena,
        )?;

        parent = Self::init_delete_plan_inner(
            parent,
            master,
            &resolved_query.table_name,
            resolved_query.root_page,
        )?;
        parent = Self::Terminate(Terminate {
            child: Box::new(parent),
        });
        Ok(PreparedPlan::new(parent, arena).with_table(&resolved_query.table_name))
    }

    fn init_delete_plan_inner(
        mut parent: Plan<V>,
        master: &Master,
        table_name: &str,
        root_page: u32,
    ) -> InkResult<Plan<V>> {
        for index in master.indexes_on(table_name)? {
            let prepare = PrepareIndex::new(index, Box::new(IndexDelete), Box::new(parent));
            parent = Plan::PrepareIndex(prepare);
        }
        parent = Self::Delete(Delete::new(Box::new(parent), root_page));
        Ok(parent)
    }
    fn init_update_plan(
        mut resolved_query: ResolvedUpdateQuery,
        master: &Master,
    ) -> InkResult<PreparedPlan<V>> {
        /*
         *
         *
         *
         * */
        let mut plan = Self::scan_with_predicate(
            resolved_query.root_page,
            &resolved_query.table_name,
            ScanMode::Safe,
            resolved_query.where_clause,
            master,
            &resolved_query.arena,
        )?;
        plan = Plan::Materialized(MaterializedResult::new(Box::new(plan)));
        plan = Self::init_delete_plan_inner(
            plan,
            master,
            &resolved_query.table_name,
            resolved_query.root_page,
        )?;
        plan = Self::Update(Update::new(Box::new(plan), resolved_query.affected_columns));
        plan = Self::init_insert_plan_inner(
            plan,
            master,
            resolved_query.table_name.clone(),
            resolved_query.root_page,
        )?;
        plan = Plan::Terminate(Terminate::new(Box::new(plan)));
        let table_name = resolved_query.table_name.clone();
        Ok(PreparedPlan::new(plan, resolved_query.arena.take()).with_table(&table_name))
    }

    fn init_create_index_plan(
        resolved_query: ResolvedCreateIndexQuery,
    ) -> InkResult<PreparedPlan<V>> {
        let mut child = Self::TableScan(TableScan::new(
            resolved_query.relation_root_page,
            ScanMode::Safe,
            resolved_query.relation_name.clone(),
        )?);
        let mut arena = ExprArena::new();
        let key = arena.push(Expr::ColumnRef(resolved_query.column_index));
        if resolved_query.is_unique {
            child = Plan::Sort(Sort::new(Box::new(child), key, false));
        }
        let rl_name = resolved_query.relation_name.clone();
        let parent = Self::CreateIndex(CreateIndex::new(Box::new(child), resolved_query)?);
        Ok(PreparedPlan::new(parent, arena).with_table(&rl_name))
    }
}

impl<V: Vfs> Plan<V> {
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        match self {
            Self::TableScan(t) => t.next(ctx),
            Self::Filter(f) => f.next(ctx),
            Self::Count(c) => c.next(ctx),
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
            Self::Sort(s) => s.next(ctx),
            Self::Update(u) => u.next(ctx),
            Self::PrepareInsert(pi) => pi.next(ctx),
            Self::Materialized(m) => m.next(ctx),
            Halt => Ok(None),
            // _ => todo!(),
        }
    }

    pub fn node_label(&self, arena: &ExprArena, table: Option<&Table>) -> String {
        match self {
            Self::TableScan(tb) => match tb.pushed_predicate() {
                Some(pushed) => format!(
                    "TableScan [root_page: {}, scan: {}, filter: {}]",
                    tb.cursor.root,
                    tb.guard.scan_type(),
                    render_expr(arena, pushed, table)
                ),
                None => format!(
                    "TableScan [root_page: {}, scan: {}]",
                    tb.cursor.root,
                    tb.guard.scan_type()
                ),
            },
            Self::Filter(f) => format!("Filter [{}]", render_expr(arena, f.predicate(), table)),
            Self::Count(c) => match c.arg() {
                Some(arg) => format!("Count [count({})]", render_expr(arena, arg, table)),
                None => "Count [count(*)]".into(),
            },
            Self::Limit(l) => format!("Limit [limit: {}]", l.limit),
            Self::Insert(i) => format!(
                "Insert [root_page: {}, key: {}, data: {} bytes]",
                i.root_page,
                i.key,
                i.data.len()
            ),
            Self::PrepareInsert(pi) => format!("PrepareInsert [rows: {:?}]", pi.rows),
            Self::PrepareRow(pr) => {
                format!("PrepareRow [root_page: {}]", pr.root_page,)
            }
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
            Self::Sort(s) => format!("Sort [i: {}]", s.id()),
            Self::Update(u) => format!(
                "Update [(columns_indexes, arena_indexes) -> {:?}]",
                u.affected_columns
            ),
            Self::Materialized(m) => "MaterializedResult".into(),
            Halt => "Halt".into(),
            _ => todo!(),
        }
    }
}

fn index_roots(master: &Master, table_name: &str) -> InkResult<Vec<u32>> {
    Ok(master
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

    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
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

    fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        let explain_table = ctx.table();
        println!("{}", PlanTree::new(&self.child, ctx.arena, explain_table));
        Ok(None)
    }
}

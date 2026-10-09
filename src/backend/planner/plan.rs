use super::super::executor::{project::Project, tablescan::TableScan};
use super::prepared_plan::PreparedPlan;
use crate::backend::analyzer::{
    ResolvedCountQuery, ResolvedCreateIndexQuery, ResolvedDeleteQuery, ResolvedDropIndexQuery,
    ResolvedDropTableQuery, ResolvedInsertQuery, ResolvedQuery, ResolvedSelectQuery,
    ResolvedUpdateQuery,
};
use crate::backend::executor::Row;
use crate::backend::executor::aggregate::Count;
use crate::backend::executor::context::ExecCtx;
use crate::backend::executor::create::{CreateIndex, CreateTable};
use crate::backend::executor::delete::Delete;
use crate::backend::executor::drop::DropTbl as DropTblExec;
use crate::backend::executor::eval::{Eval, render_expr};
use crate::backend::executor::filter::Filter;
use crate::backend::executor::index::{
    IndexDelete, IndexExactMatch, IndexInsert, IndexRangeScan, PrepareIndex,
};
use crate::backend::executor::insert::Insert;
use crate::backend::executor::limit::Limit;
use crate::backend::executor::materialized::MaterializedResult;
use crate::backend::executor::prepare::{PrepareInsert, PrepareRow};
use crate::backend::executor::rowid::{RowRangeScan, render_rowid_range};
use crate::backend::executor::scan_guard::ScanMode;
use crate::backend::executor::singlerow::SingleRow;
use crate::backend::executor::sort::Sort;
use crate::backend::executor::transaction::{
    BeginTransaction, CommitTransaction, RollBackTransaction,
};
use crate::backend::executor::truncate::TruncateTable;
use crate::backend::executor::update::Update;
use crate::backend::optimizer::optimize_index_scan;
use crate::errors::InkError;
use crate::pager::pager::Pager;
use crate::schema::Table;
use crate::sql::ast::Expr::{self, ColumnRef, StringLitteral};
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;
use crate::{InkResult, Master};

/// A tree of operators, one per node, each pulling rows from its children.
///
/// Building a plan walks a resolved query and joins operators together, and
/// running it is calling the root until it stops yielding rows. Every kind of
/// operator the engine has is a variant here.
#[derive(Debug, Default)]
pub enum Plan<V: Vfs> {
    /// Begins a transaction.
    BeginTransaction(BeginTransaction),
    /// Commits a transaction.
    CommitTransaction(CommitTransaction),
    /// Counts rows.
    Count(Count<V>),
    /// Creates and fills an index.
    CreateIndex(CreateIndex<V>),
    /// Creates a table.
    CreateTable(CreateTable<V>),
    /// Deletes rows.
    Delete(Delete<V>),
    /// Marks the schema stale after a drop.
    DropTbl(DropTblExec<V>),
    /// Prints the plan below it.
    Explain(Explain<V>),
    /// Passes on rows that satisfy a predicate.
    Filter(Filter<V>),
    /// Finds rows by an exact index value.
    IndexExactMatch(IndexExactMatch<V>),
    /// Walks an index over a range of values.
    IndexRangeScan(IndexRangeScan<V>),
    /// Inserts one cell into a tree.
    Insert(Insert<'static, V>),
    /// Hands on at most a set number of rows.
    Limit(Limit<V>),
    /// Collects its rows before handing them back.
    Materialized(MaterializedResult<V>),
    /// Applies an index change per row.
    PrepareIndex(PrepareIndex<V>),
    /// Hands out the rows of an INSERT.
    PrepareInsert(PrepareInsert<V>),
    /// Stores each row in the table.
    PrepareRow(PrepareRow<V>),
    /// Evaluates the output expressions.
    Project(Project<V>),
    /// Rolls a transaction back.
    RollbackTransaction(RollBackTransaction),
    /// Walks rows by row id range.
    RowRangeScan(RowRangeScan<V>),
    /// Yields one empty row.
    SingleRow(SingleRow),
    /// Orders its rows.
    Sort(Sort<V>),
    /// Walks a table.
    TableScan(TableScan<V>),
    /// Runs its child to the end.
    Terminate(Terminate<V>),
    /// Empties or frees a whole table.
    TruncateTable(TruncateTable<V>),
    /// Rewrites columns of each row.
    Update(Update<V>),
    /// Nothing to do, used where a child is expected but none is needed.
    #[default]
    Halt,
}
impl<V: Vfs> Plan<V> {
    /// The operators this one pulls from, with none for a leaf.
    pub fn children(&self) -> Vec<&Plan<V>> {
        match self {
            Plan::CreateIndex(ci) => ci.child().into_iter().collect(),
            Plan::Count(c) => vec![c.child()],
            Plan::Delete(d) => vec![d.child()],
            Plan::DropTbl(dt) => vec![dt.child()],
            Plan::Explain(e) => vec![e.child()],
            Plan::Filter(f) => vec![f.child()],
            Plan::Limit(l) => vec![l.child()],
            Plan::Materialized(m) => vec![m.child()],
            Plan::PrepareIndex(pi) => vec![pi.child()],
            Plan::PrepareRow(pr) => vec![pr.child()],
            Plan::Project(p) => vec![p.child()],
            Plan::Sort(s) => vec![s.child()],
            Plan::Terminate(t) => vec![t.child()],
            Plan::TruncateTable(tt) => vec![tt.child()],
            Plan::Update(u) => vec![u.child()],
            _ => Vec::new(),
        }
    }
}
/// A plan shown as a tree, indented one level per operator.
pub struct PlanTree<'a, V: Vfs> {
    plan: &'a Plan<V>,
    arena: &'a ExprArena,
    table: Option<&'a Table>,
}

impl<'a, V: Vfs> PlanTree<'a, V> {
    pub fn new(plan: &'a Plan<V>, arena: &'a ExprArena, table: Option<&'a Table>) -> Self {
        Self { plan, arena, table }
    }

    /// Write one node, then each child indented under it.
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
    /// Build a plan for a resolved query.
    ///
    /// A select or a delete scans a table with its predicate pushed into the
    /// scan, so it can become an index or rowid scan where one fits. An insert
    /// goes from the rows down to the table and up through an index operator for
    /// each index, so every row reaches all of them. An update deletes and
    /// re-inserts, with the rows collected first so the scan is not disturbed by
    /// its own changes.
    #[allow(clippy::only_used_in_recursion)]
    pub fn create_plan(
        resolved_query: ResolvedQuery,
        pager: &mut Pager<V>,
        master: &Master,
    ) -> Result<PreparedPlan<V>, InkError> {
        match resolved_query {
            ResolvedQuery::BeginTransactionQuery => Ok(PreparedPlan::direct(
                Plan::BeginTransaction(BeginTransaction),
                ExprArena::new(),
            )),
            ResolvedQuery::CommitTransactionQuery => Ok(PreparedPlan::direct(
                Plan::CommitTransaction(CommitTransaction),
                ExprArena::new(),
            )),
            ResolvedQuery::CountQuery(stmt) => Self::init_count_plan(stmt, master),
            ResolvedQuery::CreateIndexQuery(stmt) => Self::init_create_index_plan(stmt),
            ResolvedQuery::CreateTableQuery(stmt) => Ok(PreparedPlan::new(
                Plan::CreateTable(CreateTable::new(stmt)),
                ExprArena::new(),
            )),
            ResolvedQuery::DeleteQuery(stmt) => Self::init_delete_plan(stmt, master),
            ResolvedQuery::DropIndexQuery(stmt) => Self::init_drop_index_plan(stmt),
            ResolvedQuery::DropTblQuery(stmt) => Self::init_drop_table_plan(stmt),
            ResolvedQuery::ExplainQuery(stmt) => {
                let inner = Self::create_plan(*stmt.query, pager, master)?;
                let (parent, arena, table) = inner.into_parts();
                let mut prepared =
                    PreparedPlan::new(Plan::Explain(Explain::new(Box::new(parent))), arena);
                prepared.set_statement_table(table);
                Ok(prepared)
            }
            ResolvedQuery::InsertQuery(stmt) => Self::init_insert_plan(stmt, master),
            ResolvedQuery::RollbackTransactionQuery => Ok(PreparedPlan::direct(
                Plan::RollbackTransaction(RollBackTransaction),
                ExprArena::new(),
            )),
            ResolvedQuery::SelectQuery(stmt) => Self::init_select_plan(stmt, master),
            ResolvedQuery::TruncateTable(stmt) => {
                let indexes = index_roots(master, &stmt.table_name)?;
                Ok(PreparedPlan::new(
                    Plan::TruncateTable(TruncateTable::new(
                        stmt.root_page,
                        indexes.into(),
                        Box::new(Plan::<V>::Halt),
                    )),
                    ExprArena::new(),
                ))
            }
            ResolvedQuery::UpdateQuery(stmt) => Self::init_update_plan(stmt, master),
        }
    }

    /// Build a scan with its predicate pushed down, then let the optimizer try
    /// to replace it with an index or rowid scan.
    ///
    /// A filter that comes back still sitting on a plain scan had nothing that
    /// could use an index, so the filter is dropped and the scan keeps the
    /// predicate on its own.
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

    /// Build a plan for a SELECT.
    ///
    /// A SELECT with no FROM clause runs over one empty row. Ordering and the
    /// limit wrap around the scan, and the projection goes on top unless it would
    /// only hand back the columns in the order they were already in.
    fn init_select_plan(
        resolved_query: ResolvedSelectQuery,
        master: &Master,
    ) -> Result<PreparedPlan<V>, InkError> {
        let Some((table_name, root_page)) = resolved_query.table.clone() else {
            let columns = resolved_query.columns.clone();
            let parent = Self::Project(Project::new(
                Box::new(Self::SingleRow(SingleRow::new())),
                columns,
            ));
            return Ok(PreparedPlan::new(parent, resolved_query.arena));
        };
        let mode = ScanMode::Safe;
        let mut child = Self::scan_with_predicate(
            root_page,
            &table_name,
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
         * Optimization: Skip the projection when it is an identity projection,
         * meaning all columns are selected directly, in their original order,
         * without any expressions or transformations.
         *
         * Relation R: (C0, C1, C2)
         * Identity projection: (C0, C1, C2)
         */
        let columns = resolved_query.columns.clone();
        let identity = master.table(&table_name).is_some_and(|table| {
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

        Ok(PreparedPlan::new(parent, resolved_query.arena).with_table(&table_name))
    }

    /// Build a plan for a count, with any limit above the count itself.
    fn init_count_plan(
        resolved_query: ResolvedCountQuery,
        master: &Master,
    ) -> InkResult<PreparedPlan<V>> {
        let mode = ScanMode::Safe;
        let child = Self::scan_with_predicate(
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

    /// Build a plan for an INSERT, from the rows down to the table and the indexes.
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

    /// Put the row preparation, and an index operator for each index, around a
    /// plan that yields rows to insert.
    fn init_insert_plan_inner(
        mut plan: Plan<V>,
        master: &Master,
        table_name: String,
        root_page: u32,
    ) -> InkResult<Plan<V>> {
        let indexes = master.indexes_on(&table_name)?;
        plan = Plan::PrepareRow(PrepareRow::<V>::new(Box::new(plan), root_page, table_name));
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

    /// Build a plan for a DELETE.
    ///
    /// The scan is unsafe: it deletes rows as it walks, so it has to follow the
    /// row it was on rather than trust its page to stay put.
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

    /// Put an index delete for each index, and the table delete, around a plan
    /// that yields rows to remove.
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
    /// Build a plan for an UPDATE, as a delete followed by an insert.
    ///
    /// The rows are collected first, so that deleting them does not disturb the
    /// scan that is still looking for them.
    fn init_update_plan(
        mut resolved_query: ResolvedUpdateQuery,
        master: &Master,
    ) -> InkResult<PreparedPlan<V>> {
        
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

    /// Build a plan for a CREATE INDEX: a scan of the table, sorted by the
    /// indexed column when the index is unique, feeding the create.
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
    /// Build a plan for a DROP TABLE, which also frees its index trees.
    fn init_drop_table_plan(resolved_query: ResolvedDropTableQuery) -> InkResult<PreparedPlan<V>> {
        Self::init_drop_plan(
            2,
            resolved_query.tbl_name.clone(),
            resolved_query.root_page,
            resolved_query.indexes,
        )
    }

    /// Build a plan for a DROP INDEX.
    fn init_drop_index_plan(resolved_query: ResolvedDropIndexQuery) -> InkResult<PreparedPlan<V>> {
        Self::init_drop_plan(
            1,
            resolved_query.index_name.clone(),
            resolved_query.root_page,
            Vec::new(),
        )
    }
    /// The plan a drop shares: find the row being dropped in the catalog, delete
    /// it, free the pages that belonged to it, and mark the schema stale.
    ///
    /// A table is found by the table name column and an index by the index name
    /// column, and a table also carries the root pages of its indexes.
    fn init_drop_plan(
        target_col: usize,
        target_name: String,
        root_page: u32,
        indexes: Vec<u32>,
    ) -> InkResult<PreparedPlan<V>> {
        let mut plan = Self::TableScan(TableScan::new(
            1,
            ScanMode::Unsafe,
            "MASTER".into(), /*Must be unused*/
        )?);
        let mut arena = ExprArena::new();
        arena.push(ColumnRef(target_col));
        arena.push(StringLitteral(target_name));
        let index = arena.push(Expr::BinaryOp {
            left: 0,
            op: crate::sql::ast::BinaryOperator::Eq,
            right: 1,
        });
        plan = Self::Filter(Filter::new(Box::new(plan), index));
        plan = Self::Delete(Delete::new(Box::new(plan), 1));
        plan = Self::TruncateTable(TruncateTable::dropping(
            root_page,
            indexes.into(),
            Box::new(plan),
        ));
        plan = Self::DropTbl(DropTblExec::new(Box::new(plan)));

        Ok(PreparedPlan::new(plan, arena))
    }
}

impl<V: Vfs> Plan<V> {
    /// Pull the next row from this operator.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        match self {
            Self::BeginTransaction(bt) => bt.next(ctx),
            Self::CommitTransaction(ct) => ct.next(ctx),
            Self::Count(c) => c.next(ctx),
            Self::CreateIndex(ci) => ci.next(ctx),
            Self::CreateTable(c) => c.next(ctx),
            Self::Delete(d) => d.next(ctx),
            Self::DropTbl(dt) => dt.next(ctx),
            Self::Explain(e) => e.next(ctx),
            Self::Filter(f) => f.next(ctx),
            Self::IndexExactMatch(iem) => iem.next(ctx),
            Self::IndexRangeScan(irc) => irc.next(ctx),
            Self::Insert(i) => i.next(ctx),
            Self::Limit(l) => l.next(ctx),
            Self::Materialized(m) => m.next(ctx),
            Self::PrepareIndex(pi) => pi.next(ctx),
            Self::PrepareInsert(pi) => pi.next(ctx),
            Self::PrepareRow(pr) => pr.next(ctx),
            Self::Project(p) => p.next(ctx),
            Self::RollbackTransaction(rbt) => rbt.next(ctx),
            Self::RowRangeScan(rrs) => rrs.next(ctx),
            Self::SingleRow(s) => s.next(ctx),
            Self::Sort(s) => s.next(ctx),
            Self::TableScan(t) => t.next(ctx),
            Self::Terminate(t) => t.next(ctx),
            Self::TruncateTable(tb) => tb.next(ctx),
            Self::Update(u) => u.next(ctx),
            Self::Halt => Ok(None),
        }
    }

    /// One line describing this operator, with its expressions rendered.
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
            Self::RowRangeScan(r) => format!(
                "RowRangeScan [root_page: {}, rowid: {}]",
                r.root_page(),
                render_rowid_range(&r.range)
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
            Self::SingleRow(_) => "SingleRow".into(),
            Self::Update(u) => format!(
                "Update [(columns_indexes, arena_indexes) -> {:?}]",
                u.affected_columns
            ),
            Self::Materialized(_) => "MaterializedResult".into(),
            Self::Halt => "Halt".into(),
            Self::DropTbl(_) => "Drop Table".into(),
        }
    }
}

/// The root page of every index on a table.
pub(crate) fn index_roots(master: &Master, table_name: &str) -> InkResult<Vec<u32>> {
    Ok(master
        .indexes_on(table_name)?
        .into_iter()
        .map(|index| index.index_root_page)
        .collect())
}

/// Runs its child to the end and hands back nothing.
///
/// A statement that changes rows has no result set, but its plan still has to be
/// walked from top to bottom for the work to happen.
#[derive(Debug)]
pub struct Terminate<V: Vfs> {
    child: Box<Plan<V>>,
}

impl<V: Vfs> Terminate<V> {
    pub fn new(child: Box<Plan<V>>) -> Self {
        Self { child }
    }

    /// The plan being run.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }

    /// Run the child to the end.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        while self.child.next(ctx)?.is_some() {}
        Ok(None)
    }
}

/// Prints the plan below it instead of running it.
#[derive(Debug)]
pub struct Explain<V: Vfs> {
    child: Box<Plan<V>>,
}

impl<V: Vfs> Explain<V> {
    fn new(child: Box<Plan<V>>) -> Self {
        Self { child }
    }

    /// The plan being explained.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }

    fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        let explain_table = ctx.table();
        println!("{}", PlanTree::new(&self.child, ctx.arena, explain_table));
        Ok(None)
    }
}

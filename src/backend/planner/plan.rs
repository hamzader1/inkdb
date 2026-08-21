use super::super::executor::{project::Project, scan::TableScan};
use crate::backend::analyze::{ResolvedInsertQuery, ResolvedQuery, ResolvedSelectQuery};
use crate::backend::executor::Row;
use crate::backend::executor::create::CreateTable;
use crate::backend::executor::eval::Eval;
use crate::backend::executor::filter::Filter;
use crate::backend::executor::insert::Insert;
use crate::backend::executor::limit::Limit;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;

#[derive(Debug)]
pub enum Plan {
    TableScan(TableScan),
    Filter(Filter),
    Limit(Limit),
    Project(Project),
    Insert(Insert),
    CreateTable(CreateTable),
}

#[derive(Debug)]
pub struct Arena {
    parent: Plan,
    arena: ExprArena,
}
impl Arena {
    pub fn new(parent: Plan, arena: ExprArena) -> Self {
        Self { parent, arena }
    }
    pub fn next(&mut self, pager: &mut Pager) -> Result<Option<Row>, SqliteError> {
        self.parent.next(pager, &self.arena)
    }
}

impl Plan {
    pub fn create_plan(
        resolved_query: ResolvedQuery,
        pager: &mut Pager,
    ) -> Result<Arena, SqliteError> {
        match resolved_query {
            ResolvedQuery::SelectQuery(stmt) => Self::initialize_select_plan(stmt, pager),
            ResolvedQuery::InsertQuery(stmt) => Self::initialize_insert_plan(stmt),
            ResolvedQuery::CreateTableQuery(stmt) => Ok(Arena::new(
                Plan::CreateTable(CreateTable::new(stmt)),
                ExprArena::new(),
            )),
            _ => todo!(), // ResolvedQuery::CreateTableQuery(stmt) => Ok(ResolvedQuery::
        }
    }

    pub fn initialize_select_plan(
        resolved_query: ResolvedSelectQuery,
        pager: &mut Pager,
    ) -> Result<Arena, SqliteError> {
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

        Ok(Arena::new(parent, resolved_query.arena))
    }

    pub fn initialize_insert_plan(
        resolved_query: ResolvedInsertQuery,
    ) -> Result<Arena, SqliteError> {
        let plan = Plan::Insert(Insert::new(
            resolved_query.root,
            resolved_query.values,
            resolved_query.entry_hint,
        ));
        Ok(Arena {
            parent: plan,
            arena: ExprArena::new(),
        })
    }
}

impl Plan {
    pub fn next(
        &mut self,
        pager: &mut Pager,
        arena: &ExprArena,
    ) -> Result<Option<Row>, SqliteError> {
        match self {
            Self::TableScan(t) => t.next(pager),
            Self::Filter(f) => f.next(pager, arena),
            Self::Limit(l) => l.next(pager, arena),
            Self::Project(p) => p.next(pager, arena),
            Self::Insert(i) => i.next(pager),
            Self::CreateTable(c) => c.next(pager),
            _ => todo!(),
        }
    }
}

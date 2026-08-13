use super::super::executor::{project::Project, scan::TableScan};
use crate::backend::analyze::ResolvedQuery;
use crate::backend::executor::{Executor, Row};
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::sql::parser::Arena;
use crate::vfs::file::SqliteFile;

pub enum Plan {
    TableScan {
        root_page: u32,
    },
    Project {
        input: Box<Plan>,
        arena: Arena,
        output: Vec<usize>,
    },
}

impl Plan {
    pub fn create_plan(resolved_query: ResolvedQuery) -> Plan {
        let table = Self::TableScan {
            root_page: resolved_query.root_page,
        };
        // TODO: missing where

        Self::Project {
            input: Box::new(table),
            arena: resolved_query.arena,
            output: resolved_query.columns,
        }
    }
}
pub fn build_executor(plan: &Plan, pager: &mut Pager) -> Box<dyn Executor> {
    match plan {
        Plan::TableScan { root_page } => Box::new(TableScan::new(*root_page, pager)),

        Plan::Project {
            input,
            arena,
            output,
        } => {
            let child = build_executor(input, pager);

            Box::new(Project::new(child, arena.clone(), output.clone()))
        }
    }
}

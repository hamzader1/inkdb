use super::super::executor::{project::Project, scan::TableScan};
use crate::backend::analyze::{MultiIndexColumn, ResolvedQuery};
use crate::backend::executor::Executor;
use crate::sql::parser::Arena;

enum Plan {
    TableScan {
        root_page: u32,
    },
    Project {
        input: Box<Plan>,
        arena: Arena,
        output: Vec<MultiIndexColumn>,
    },
}

impl Plan {
    fn create_plan(resolved_query: ResolvedQuery) -> Plan {
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
    // fn build_executor(&self) -> Box<dyn Executor> {
    //     match self {

    //     }
    // }
}

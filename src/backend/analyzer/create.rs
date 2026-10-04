use crate::InkResult;
use crate::backend::analyzer::ResolvedCreateIndexQuery;
use crate::backend::executor::eval::Eval;
use crate::errors::InkError;
use crate::sql::ast::{Constraint, CreateIndexStmt, CreateTableStmt, DefaultValue};
use crate::util::assert_with_runtime_err;

use super::{Analyze, ResolvedCreateTableQuery, ResolvedQuery};

impl<'a> Analyze<'a> {
    pub(crate) fn analyze_create_table_stmt(
        &self,
        mut stmt: CreateTableStmt,
    ) -> Result<ResolvedQuery, InkError> {
        if self.get_non_master_table(&stmt.name).is_ok() {
            return Err(InkError::TableAlreadyExists(stmt.name));
        }
        assert_single_primary_key(&stmt)?;
        let mut arena = stmt.arena.take();
        for idx in stmt.tbl_constraints.iter() {
            Self::fast_bind(&stmt, *idx, &mut arena)?;
        }
        let mut unique_cols = Vec::new();
        for (i, column) in stmt.columns.iter_mut().enumerate() {
            if let Some(ref mut default) = column.default {
                assert!(matches!(default, DefaultValue::Node(_)));
                let node = {
                    let DefaultValue::Node(default_index) = default else {
                        unreachable!()
                    };
                    default_index
                };
                let value = Eval::eval(&arena, *node, None)?.into_static();
                *default = DefaultValue::Val(value);
            }
            if column.is_unique() {
                unique_cols.push(i);
            }
        }
        stmt.arena = arena;

        Ok(ResolvedQuery::CreateTableQuery(ResolvedCreateTableQuery {
            meta: stmt,
            unique_on: unique_cols,
        }))
    }

    pub(crate) fn analyze_create_index_stmt(
        &self,
        stmt: CreateIndexStmt,
    ) -> InkResult<ResolvedQuery> {
        let relation = self.get_non_master_table(&stmt.table)?;
        if self.master.indexes().contains_key(&stmt.name) {
            return Err(InkError::runtime(format!(
                "Index with name {} already exists",
                stmt.name
            )));
        }
        // single col only for now
        if stmt.columns.is_empty() {
            return Err(InkError::runtime(
                "Index can be created on empty column set",
            ));
        }
        let column_index = match relation.get_col_idx(&stmt.columns[0]) {
            Some(idx) => idx,
            None => return Err(InkError::UnknownColumn(stmt.columns[0].to_string())),
        };

        Ok(ResolvedQuery::CreateIndexQuery(ResolvedCreateIndexQuery {
            query: Some(stmt.query),
            relation_root_page: relation.root_page(),
            relation_name: relation.name().clone(),
            index_name: stmt.name,
            column_index,
            is_unique: stmt.unique,
        }))
    }
}

fn assert_single_primary_key(stmt: &CreateTableStmt) -> InkResult<()> {
    let count = stmt
        .columns
        .iter()
        .filter(|column| {
            column
                .constraints
                .as_ref()
                .is_some_and(|constraints| constraints.contains(&Constraint::PrimaryKey))
        })
        .count();
    assert_with_runtime_err(count <= 1, || {
        format!("table {} has more than one primary key", stmt.name)
    })
}

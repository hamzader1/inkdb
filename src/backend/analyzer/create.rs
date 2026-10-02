use crate::backend::analyzer::ResolvedCreateIndexQuery;
use crate::backend::executor::eval::Eval;
use crate::errors::InkError;
use crate::sql::ast::{CreateIndex, CreateTableStmt, DefaultValue};
use crate::{InkResult, Master};

use super::{Analyze, ResolvedCreateTableQuery, ResolvedQuery};

impl<'a> Analyze<'a> {
    pub fn analyze_create_table_stmt(
        &self,
        mut stmt: CreateTableStmt,
    ) -> Result<ResolvedQuery, InkError> {
        if self.get_non_master_table(&stmt.name).is_ok() {
            return Err(InkError::TableAlreadyExists(stmt.name));
        }
        let mut arena = stmt.arena.take();
        for idx in stmt.tbl_constraints.iter() {
            Self::fast_bind(&stmt, *idx, &mut arena)?;
        }
        for column in stmt.columns.iter_mut() {
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
        }
        stmt.arena = arena;

        Ok(ResolvedQuery::CreateTableQuery(ResolvedCreateTableQuery {
            meta: stmt,
        }))
    }

    pub fn analyze_create_index_stmt(&self, stmt: CreateIndex) -> InkResult<ResolvedQuery> {
        let relation = self.get_non_master_table(&stmt.table)?;
        if self.master.indexes.contains_key(&stmt.name) {
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
            query: stmt.query,
            relation_root_page: relation.root_page,
            relation_name: relation.name.clone(),
            index_name: stmt.name,
            column_index,
            is_unique: stmt.unique,
        }))
    }
}

use crate::Master;
use crate::errors::SqliteError;
use crate::schema::MASTER;

use crate::sql::ast::{Expr, SelectStmt};
use crate::sql::parser::ExprArena;

use super::{Analyze, ResolvedCountQuery, ResolvedQuery, ResolvedSelectQuery};

impl Analyze {
    pub fn analyze_select_stmt(
        select_stmt: SelectStmt,
        master: &Master,
    ) -> Result<ResolvedQuery, SqliteError> {
        let SelectStmt {
            table_name,
            mut arena,
            columns,
            mut where_clause,
            mut limit,
        } = select_stmt.clone();

        let table = {
            if table_name == "master" {
                &*MASTER
            } else {
                Self::get_table(master, &table_name)?
            }
        };
        if columns.len() == 1
            && let Expr::Count { arg } = arena.nodes[columns[0]]
        {
            if let Some(arg) = arg {
                Analyze::fast_bind(table, arg, &mut arena)?;
            }
            if let Some(predicate) = where_clause {
                Analyze::fast_bind(table, predicate, &mut arena)?;
            }
            if let Some(limit_expr) = limit {
                Analyze::fast_bind(table, limit_expr, &mut arena)?;
            }
            return Ok(ResolvedQuery::CountQuery(ResolvedCountQuery {
                table_name: table.name.clone(),
                root_page: table.root_page,
                arena,
                arg,
                where_clause,
                limit,
            }));
        }
        if arena
            .nodes
            .iter()
            .any(|node| matches!(node, Expr::Count { .. }))
        {
            return Err(SqliteError::runtime(
                "count() cannot be combined with other columns yet",
            ));
        }

        let has_star = arena.nodes.contains(&Expr::Star);
        // TODO: THIS NEEDS OPTIMAZATION
        if !has_star {
            for idx in columns.iter() {
                // Analyze:
                Analyze::fast_bind(table, *idx, &mut arena)?;
            }
            if let Some(predicate) = where_clause {
                Analyze::fast_bind(table, predicate, &mut arena)?;
            }
            if let Some(limit) = limit {
                Analyze::fast_bind(table, limit, &mut arena)?;
            }
            let stmt = ResolvedSelectQuery {
                table_name,
                root_page: table.root_page,
                arena,
                columns,
                where_clause,
                limit,
            };
            return Ok(ResolvedQuery::SelectQuery(stmt));
        }
        let mut new_arena: Vec<Expr> = Vec::new();
        let mut map = vec![0; arena.nodes.len()];
        let mut new_cols = Vec::new();
        for idx in columns.iter() {
            if let &Expr::Star = &arena.nodes[*idx] {
            } else {
                new_cols.push(*idx);
            }

            Analyze::slow_bind(
                table,
                *idx,
                &mut arena,
                &mut new_arena,
                &mut map,
                &mut new_cols,
            )?;
        }
        let mut col_id = 0usize;
        for idx in columns.iter() {
            if let &Expr::Star = &arena.nodes[*idx] {
                col_id += table.get_cols_len();
            } else {
                new_cols[col_id] = map[*idx];
                col_id += 1;
            }
        }
        // TODO: Can we optimize this further to call 'slow_bind' once?
        if let Some(ref mut predicate) = where_clause {
            Analyze::slow_bind(
                table,
                *predicate,
                &mut arena,
                &mut new_arena,
                &mut map,
                &mut new_cols,
            )?;
            *predicate = map[*predicate];
        }

        // TODO: Can we optimize this further to call 'slow_bind' once?
        if let Some(ref mut limit) = limit {
            Analyze::slow_bind(
                table,
                *limit,
                &mut arena,
                &mut new_arena,
                &mut map,
                &mut new_cols,
            )?;
            *limit = map[*limit];
        }
        let stmt = ResolvedSelectQuery {
            table_name,
            root_page: table.root_page,
            arena: ExprArena { nodes: new_arena },
            columns: new_cols,
            where_clause,
            limit,
        };

        Ok(ResolvedQuery::SelectQuery(stmt))
    }
}

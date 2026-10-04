use crate::errors::InkError;

use crate::sql::ast::{Expr, SelectStmt};
use crate::sql::parser::ExprArena;

use super::{Analyze, ResolvedCountQuery, ResolvedQuery, ResolvedSelectQuery};

impl<'a> Analyze<'a> {
    pub(crate) fn analyze_select_stmt(
        &self,
        select_stmt: SelectStmt,
    ) -> Result<ResolvedQuery, InkError> {
        let SelectStmt {
            table_name,
            mut arena,
            columns,
            mut where_clause,
            mut limit,
            mut orderby,
        } = select_stmt.clone();

        let Some(table_name) = table_name else {
            if arena
                .nodes
                .iter()
                .any(|node| matches!(node, Expr::Star) || matches!(node, Expr::Identifier(_)))
            {
                return Err(InkError::runtime(
                    "SELECTing columns requires a FROM clause",
                ));
            }
            if arena
                .nodes
                .iter()
                .any(|node| matches!(node, Expr::Count { .. }))
            {
                return Err(InkError::runtime("count() requires a FROM clause"));
            }
            return Ok(ResolvedQuery::SelectQuery(ResolvedSelectQuery {
                table: None,
                arena,
                columns: columns.into(),
                where_clause,
                limit,
                orderby,
            }));
        };
        let table = { self.get_table(&table_name)? };
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
                table_name: table.name().clone(),
                root_page: table.root_page(),
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
            return Err(InkError::runtime(
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
            if let Some(orderby) = orderby {
                Analyze::fast_bind(table, orderby.index, &mut arena)?;
            }
            let stmt = ResolvedSelectQuery {
                table: Some((table_name, table.root_page())),
                arena,
                columns: columns.into(),
                where_clause,
                limit,
                orderby,
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

        if let Some(ref mut orderby) = orderby {
            Analyze::slow_bind(
                table,
                orderby.index,
                &mut arena,
                &mut new_arena,
                &mut map,
                &mut new_cols,
            )?;
            orderby.index = map[orderby.index];
        }
        let stmt = ResolvedSelectQuery {
            table: Some((table_name, table.root_page())),
            arena: ExprArena { nodes: new_arena },
            columns: new_cols.into(),
            where_clause,
            limit,
            orderby,
        };

        Ok(ResolvedQuery::SelectQuery(stmt))
    }
}

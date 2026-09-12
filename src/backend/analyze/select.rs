use crate::errors::SqliteError;
use crate::schema::{SQLITE_MASTER, Table};
use crate::util::sqlite_assert_with_runtime_err;
use crate::{SqliteMaster, SqliteResult};

use crate::sql::ast::{Expr, SelectStmt};
use crate::sql::parser::ExprArena;

use super::{Analyze, ResolvedQuery, ResolvedSelectQuery};

impl Analyze {
    pub fn analyze_select_stmt(
        select_stmt: SelectStmt,
        sqlite_master: &SqliteMaster,
    ) -> Result<ResolvedQuery, SqliteError> {
        let SelectStmt {
            table_name,
            mut arena,
            mut columns,
            mut where_clause,
            mut limit,
        } = select_stmt.clone();

        let table = {
            if table_name == "sqlite_master" {
                &*SQLITE_MASTER
            } else {
                Self::get_table(sqlite_master, &table_name)?
            }
        };
        // if table.name == "sqlite_master" {
        //     return Self::handle_sqlite_master_query(select_stmt, sqlite_master);
        // }

        let has_star = arena
            .nodes
            .iter()
            .find(|node| *node == &Expr::Star)
            .is_some();

        // TODO: THIS NEEDS OPTIMAZATION
        if !has_star {
            for idx in columns.iter() {
                // Analyze:
                Analyze::fast_bind(table, *idx, &mut arena)?;
            }
            if let Some(predict) = where_clause {
                Analyze::fast_bind(table, predict, &mut arena)?;
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
        // TODO: Can we optimaze this further to call 'slow_bind' once?
        if let Some(ref mut predict) = where_clause {
            Analyze::slow_bind(
                table,
                *predict,
                &mut arena,
                &mut new_arena,
                &mut map,
                &mut new_cols,
            )?;
            *predict = map[*predict];
        }

        // TODO: Can we optimaze this further to call 'slow_bind' once?
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

    // fn handle_sqlite_master_query(
    //     select_stmt: SelectStmt,
    //     sqlite_master: &SqliteMaster,
    // ) -> SqliteResult<ResolvedQuery> {
    //     sqlite_assert_with_runtime_err(
    //         select_stmt.table_name == "sqlite_master",
    //         &format!(
    //             "sqlite_master_hanlder called with table {}",
    //             select_stmt.table_name
    //         ),
    //     );
    //     todo!()
    // }
}

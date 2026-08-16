use crate::SqliteMaster;
use crate::errors::SqliteError;
use crate::schema::Table;
use crate::sql::ast::{Expr, SelectStmt};
use crate::sql::parser::ExprArena;

type ColumnIndex = usize;
type ArenaColumnIndex = usize;

pub struct Analyze;
// #[derive(Debug)]
// pub struct MultiIndexColumn {
//     pub col_idx: ColumnIndex,
//     pub arena_col_idx: ArenaColumnIndex,
// }
#[derive(Debug)]
pub struct ResolvedQuery {
    pub root_page: u32,
    pub arena: ExprArena,
    pub columns: Vec<usize>,
    pub where_clause: Option<usize>,
    pub limit: Option<usize>,
}

// impl MultiIndexColumn {
//     fn new(col_idx: usize, arena_col_idx: usize) -> Self {
//         Self {
//             col_idx,
//             arena_col_idx,
//         }
//     }
// }

impl Analyze {
    pub fn analyze(
        select_stmt: SelectStmt,
        sqlite_master: &SqliteMaster,
    ) -> Result<ResolvedQuery, SqliteError> {
        let SelectStmt {
            table_name,
            mut arena,
            columns,
            mut where_clause,
            mut limit,
        } = select_stmt;
        let table = match sqlite_master.tables.get(&table_name) {
            Some(table) => table,
            _ => {
                return Err(SqliteError::RuntimeError(format!(
                    "Table {} does not exists",
                    table_name
                )));
            }
        };

        let has_star = arena
            .nodes
            .iter()
            .find(|node| *node == &Expr::Star)
            .is_some();

        // TODO: THIS NEEDS OPTIMAZATION
        if !has_star {
            for idx in columns.iter() {
                Analyze::fast_bind(table, *idx, &mut arena)?;
            }
            if let Some(predict) = where_clause {
                Analyze::fast_bind(table, predict, &mut arena)?;
            }
            if let Some(limit) = limit {
                Analyze::fast_bind(table, limit, &mut arena);
            }
            return Ok(ResolvedQuery {
                root_page: table.root_page,
                arena,
                columns,
                where_clause,
                limit,
            });
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

        Ok(ResolvedQuery {
            root_page: table.root_page,
            arena: ExprArena { nodes: new_arena },
            columns: new_cols,
            where_clause,
            limit,
        })
    }

    fn slow_bind(
        table: &Table,
        idx: usize,
        arena: &mut ExprArena,
        new: &mut Vec<Expr>,
        map: &mut Vec<usize>,
        new_cols: &mut Vec<usize>,
    ) -> Result<(), SqliteError> {
        let expr = &arena.nodes[idx];
        match expr {
            Expr::Identifier(col_name) => match table.get_col_idx(col_name) {
                Some(col_idx) => {
                    new.push(Expr::ColumnRef(col_idx));
                    map[idx] = new.len() - 1;
                    return Ok(());
                }
                _ => {
                    return Err(SqliteError::RuntimeError(format!(
                        "Column {} does not exists",
                        col_name
                    )));
                }
            },
            Expr::Star => {
                for i in 0..table.get_cols_len() {
                    new.push(Expr::ColumnRef(i));
                    new_cols.push(new.len() - 1);
                }
                map[idx] = new.len() - 1;
            }
            Expr::Number(_) | Expr::Float(_) | Expr::Bool(_) | Expr::StringLitteral(_) => {
                new.push(expr.clone());
                map[idx] = new.len() - 1;
                return Ok(());
            }
            Expr::Add(left, right)
            | Expr::Substract(left, right)
            | Expr::Multiply(left, right)
            | Expr::Devide(left, right) => {
                let left = *left;
                let right = *right;

                let node = expr.clone();
                Self::slow_bind(table, left, arena, new, map, new_cols)?;
                Self::slow_bind(table, right, arena, new, map, new_cols)?;
                new.push(Expr::remap_l_r(&node, map[left], map[right]));
                map[idx] = new.len() - 1;
            }
            Expr::Neg(i) => {
                let child = *i;
                Self::slow_bind(table, child, arena, new, map, new_cols)?;
                new.push(Expr::Neg(map[child]));
                map[idx] = new.len() - 1;
            }
            Expr::Not(i) => {
                let child = *i;
                Self::slow_bind(table, child, arena, new, map, new_cols)?;
                new.push(Expr::Not(map[child]));
                map[idx] = new.len() - 1;
            }
            Expr::And { left, right } => {
                let left = *left;
                let right = *right;

                let node = expr.clone();
                Self::slow_bind(table, left, arena, new, map, new_cols)?;
                Self::slow_bind(table, right, arena, new, map, new_cols)?;
                new.push(Expr::remap_l_r(&node, map[left], map[right]));
                map[idx] = new.len() - 1;
            }
            Expr::Or { left, right } => {
                let left = *left;
                let right = *right;

                let node = expr.clone();
                Self::slow_bind(table, left, arena, new, map, new_cols)?;
                Self::slow_bind(table, right, arena, new, map, new_cols)?;
                new.push(Expr::remap_l_r(&node, map[left], map[right]));
                map[idx] = new.len() - 1;
            }
            Expr::BinaryOp { left, op, right } => {
                let left = *left;
                let right = *right;

                let node = expr.clone();
                Self::slow_bind(table, left, arena, new, map, new_cols)?;
                Self::slow_bind(table, right, arena, new, map, new_cols)?;
                new.push(Expr::remap_l_r(&node, map[left], map[right]));
                map[idx] = new.len() - 1;
            }
            _ => {
                return Err(SqliteError::RuntimeError(
                    "Expression is not allowed in select statement list".into(),
                ));
            }
        }
        Ok(())
    }

    // General purpose
    fn fast_bind(table: &Table, idx: usize, arena: &mut ExprArena) -> Result<(), SqliteError> {
        let expr = &arena.nodes[idx];
        match expr {
            Expr::Identifier(col_name) => match table.get_col_idx(col_name) {
                Some(col_idx) => {
                    arena.nodes[idx] = Expr::ColumnRef(col_idx);
                    return Ok(());
                }
                _ => {
                    return Err(SqliteError::RuntimeError(format!(
                        "Column {} does not exists",
                        col_name
                    )));
                }
            },
            Expr::Number(_) | Expr::Float(_) | Expr::Bool(_) | Expr::StringLitteral(_) => {
                return Ok(());
            }
            Expr::Add(left, right) => {
                let left = *left;
                let right = *right;
                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Substract(left, right) => {
                let left = *left;
                let right = *right;
                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Multiply(left, right) => {
                let left = *left;
                let right = *right;
                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Devide(left, right) => {
                let left = *left;
                let right = *right;
                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Neg(i) => {
                Self::fast_bind(table, *i, arena)?;
            }
            Expr::Not(i) => {
                Self::fast_bind(table, *i, arena)?;
            }
            Expr::And { left, right } => {
                let left = *left;
                let right = *right;

                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::Or { left, right } => {
                let left = *left;
                let right = *right;

                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            Expr::BinaryOp { left, op, right } => {
                let left = *left;
                let right = *right;

                Self::fast_bind(table, left, arena)?;
                Self::fast_bind(table, right, arena)?;
            }
            _ => {
                return Err(SqliteError::RuntimeError(
                    "Expression is not allowed in select statement list".into(),
                ));
            }
        }
        Ok(())
    }
}

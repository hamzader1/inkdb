use super::planner::plan::{self, Plan};
use crate::backend::executor::eval::Eval;
use crate::backend::executor::index::IndexExactMatch;
use crate::errors::SqliteError;
use crate::record::Value;
use crate::sql::ast::{BinaryOperator, Expr};
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;
use crate::{SqliteMaster, SqliteResult};

pub struct Optimazer;

impl Optimazer {
    pub fn optimaze_select<F: SqliteFile>(
        plan: &mut Plan<F>,
        sqlite_master: &SqliteMaster,
        table_name: &str,
        root_page: u32,
        arena: &ExprArena,
    ) -> Result<(), SqliteError> {
        let mut plan = plan;
        while let Some(child) = plan.child_mut() {
            if child.is_filter() {
                match Self::optimaze_where(child, arena, sqlite_master, table_name, root_page)? {
                    Some(x) => {
                        let new_child = Plan::IndexExactMatch(IndexExactMatch::new(x.0, x.1, x.2));
                        *child = new_child;
                    }
                    _ => break,
                }
            }
            plan = child;
        }
        Ok(())
    }

    fn optimaze_where<F: SqliteFile>(
        plan: &Plan<F>,
        arena: &ExprArena,
        sqlite_master: &SqliteMaster,
        table_name: &str,
        relation_root_page: u32,
    ) -> SqliteResult<Option<(u32, u32, Value<'static>)>> {
        assert!(matches!(plan, Plan::Filter(_)));
        let filter_expr_index = match plan {
            Plan::Filter(p) => p.predicate(),
            _ => unreachable!(),
        };
        match arena.nodes[filter_expr_index] {
            Expr::BinaryOp { left, op, right } => {
                match Self::try_index_exact_match(
                    left,
                    right,
                    op,
                    arena,
                    sqlite_master,
                    table_name,
                    relation_root_page,
                )? {
                    Some(x) => return Ok(Some(x)),
                    None => {
                        match Self::try_index_exact_match(
                            right,
                            left,
                            op,
                            arena,
                            sqlite_master,
                            table_name,
                            relation_root_page,
                        )? {
                            Some(x) => return Ok(Some(x)),
                            None => return Ok(None),
                        }
                    }
                }
            }
            _ => unreachable!(),
        }
        todo!()
    }
    fn try_index_exact_match(
        left: usize,
        right: usize,
        op: BinaryOperator,
        arena: &ExprArena,
        sqlite_master: &SqliteMaster,
        table_name: &str,
        relation_root_page: u32,
    ) -> SqliteResult<Option<(u32, u32, Value<'static>)>> {
        if let Expr::ColumnRef(i) = arena.nodes[left]
            && let BinaryOperator::Eq = op
        {
            // right must be const
            let target = match Self::try_cast_to_const_expr(arena, right) {
                Some(value) => value,
                _ => return Ok(None),
            };
            let relation = sqlite_master.tables.get(table_name).unwrap();
            let col = relation.get_col_name(i).unwrap();

            let mut index_root_page = None;
            for index in sqlite_master.indexes.values() {
                if index.has_index_on(&col.name, table_name) {
                    index_root_page = Some(index.root_page);
                    break;
                }
            }
            if let Some(index_root_page) = index_root_page {
                return Ok(Some((index_root_page, relation_root_page, target)));
            }
        }

        Ok(None)
    }
    fn try_cast_to_const_expr(arena: &ExprArena, index: usize) -> Option<Value<'static>> {
        Eval::eval(arena, index, None).ok()
    }
}

use std::ops::Bound::Included;
use std::ops::{Bound, RangeBounds};

use super::executor::index::IndexRangeScan;
use super::executor::scan_guard::{CustomScanGuard, ScanGuard};
use super::planner::plan::Plan;
use crate::backend::executor::eval::Eval;
use crate::backend::executor::index::IndexExactMatch;
use crate::backend::executor::scan_guard::{SafeScan, UnsafeScan};
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::sql::ast::{BinaryOperator, Expr};
use crate::sql::parser::ExprArena;
use crate::vfs::file::SqliteFile;
use crate::{SqliteMaster, SqliteResult};

pub struct Optimizer<'a, F: SqliteFile, G>
where
    G: FnOnce() -> Box<dyn ScanGuard<F>>,
{
    sqlite_master: &'a SqliteMaster,
    relation: &'a crate::schema::Table,
    plan: &'a mut Plan<F>,
    pager: &'a mut Pager<F>,
    arena: &'a ExprArena,
    // if len == requested_len we have a ready index
    ready_index: Option<Plan<F>>,
    guard: CustomScanGuard<G, F>,
    is_done: bool,
}

impl<'a, F: SqliteFile, G> Optimizer<'a, F, G>
where
    G: FnOnce() -> Box<dyn ScanGuard<F>>,
{
    pub fn new(
        plan: &'a mut Plan<F>,
        pager: &'a mut Pager<F>,
        sqlite_master: &'a SqliteMaster,
        table_name: &str,
        root_page: u32,
        arena: &'a ExprArena,
        // Allows the optimizer to modify the source plans to be either SafeScan or Unsafe.
        guard: CustomScanGuard<G, F>,
    ) -> Self {
        debug_assert!(
            matches!(plan, Plan::Filter(_)),
            "Expected Filter plan, found {:?}",
            plan
        );
        // already verified from the analyzing phase
        let table = sqlite_master.tables.get(table_name).unwrap();
        Self {
            sqlite_master,
            relation: table,
            plan,
            pager,
            arena,
            ready_index: None,
            guard,
            is_done: false,
        }
    }

    pub fn optimize(&mut self) -> SqliteResult<()> {
        let Plan::Filter(filter) = self.plan else {
            unreachable!()
        };

        let predict = filter.predicate();
        self.optimaze_where(predict)?;

        if self.is_done {
            return Ok(());
        }
        let Some(new_plan) = self.ready_index.take() else {
            return Ok(());
        };
        match new_plan {
            Plan::IndexExactMatch(_) | Plan::IndexRangeScan(_) => {
                *self.plan.child_mut().unwrap() = new_plan;
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    pub fn optimaze_where(&mut self, predict: usize) -> SqliteResult<()> {
        if self.is_done {
            return Ok(());
        }
        match self.arena.nodes[predict] {
            Expr::BinaryOp { left, op, right } => match op {
                BinaryOperator::NotEq => return Ok(()),
                _ => {
                    match self.try_index(left, right, op)? {
                        Some(_) => return Ok(()),
                        None => self.try_index(right, left, op),
                    };
                }
            },
            Expr::Or { .. } => {
                let _ = self.ready_index.take(); // burn the index
                self.is_done = true;
                return Ok(());
            }
            Expr::And { left, right } => {
                self.optimaze_where(left)?;
                self.optimaze_where(right)?;
            }
            _ => unreachable!(),
        };
        Ok(())
    }

    fn try_index(
        &mut self,
        left: usize,
        right: usize,
        op: BinaryOperator,
    ) -> SqliteResult<Option<()>> {
        if let Expr::ColumnRef(i) = self.arena.nodes[left]
        // && let BinaryOperator::Eq = op
        {
            // Right must be const
            let target = match Self::try_cast_to_const_expr(self.arena, right) {
                Some(value) => value,
                _ => return Ok(None),
            };
            let col = self.relation.get_col_name(i).unwrap();

            let mut index_root_page = None;
            for index in self.sqlite_master.indexes.values() {
                if index.is_on(&col.name, &self.relation.name) {
                    index_root_page = Some(index.root_page);
                    break;
                }
            }

            if !self.is_done /*&& self.ready_index.is_none()*/
            && let Some(index_root_page) = index_root_page
            {
                let index_plan = match op {
                    BinaryOperator::Eq => Plan::IndexExactMatch(IndexExactMatch::<F>::new(
                        self.pager,
                        index_root_page,
                        self.relation.root_page,
                        target,
                        self.guard.take(),
                    )?),

                    BinaryOperator::Ge => {
                        if let Some(Plan::IndexRangeScan(irc)) = self.ready_index.as_mut()
                            && irc.index_root_page() == index_root_page
                        {
                            tighten_lower_bound(&mut irc.range.0, Bound::Included(target));
                            return Ok(None);
                        }

                        self.new_index_range_scan(
                            index_root_page,
                            Bound::Included(target),
                            Bound::Unbounded,
                        )?
                    }

                    BinaryOperator::Gt => {
                        if let Some(Plan::IndexRangeScan(irc)) = self.ready_index.as_mut()
                            && irc.index_root_page() == index_root_page
                        {
                            tighten_lower_bound(&mut irc.range.0, Bound::Excluded(target));
                            return Ok(None);
                        }

                        self.new_index_range_scan(
                            index_root_page,
                            Bound::Excluded(target),
                            Bound::Unbounded,
                        )?
                    }

                    BinaryOperator::Le => {
                        if let Some(Plan::IndexRangeScan(irc)) = self.ready_index.as_mut()
                            && irc.index_root_page() == index_root_page
                        {
                            tighten_upper_bound(&mut irc.range.1, Bound::Included(target));
                            return Ok(None);
                        }

                        self.new_index_range_scan(
                            index_root_page,
                            Bound::Unbounded,
                            Bound::Included(target),
                        )?
                    }

                    BinaryOperator::Lt => {
                        if let Some(Plan::IndexRangeScan(irc)) = self.ready_index.as_mut()
                            && irc.index_root_page() == index_root_page
                        {
                            tighten_upper_bound(&mut irc.range.1, Bound::Excluded(target));
                            return Ok(None);
                        }

                        self.new_index_range_scan(
                            index_root_page,
                            Bound::Unbounded,
                            Bound::Excluded(target),
                        )?
                    }
                    _ => unreachable!(),
                };
                self.ready_index = Some(index_plan);
                return Ok(Some(()));
            }
        }

        Ok(None)
    }
    fn try_cast_to_const_expr(arena: &ExprArena, index: usize) -> Option<Value<'static>> {
        Eval::eval(arena, index, None).ok()
    }

    pub fn new_index_exact_match(
        &mut self,
        index_root_page: u32,
        target: Value<'static>,
    ) -> SqliteResult<Plan<F>> {
        Ok(Plan::IndexExactMatch(IndexExactMatch::new(
            self.pager,
            index_root_page,
            self.relation.root_page,
            target,
            self.guard.take(),
        )?))
    }
    pub fn new_index_range_scan(
        &mut self,
        index_root_page: u32,
        start: Bound<Value<'static>>,
        end: Bound<Value<'static>>,
    ) -> SqliteResult<Plan<F>> {
        Ok(Plan::IndexRangeScan(IndexRangeScan::new(
            index_root_page,
            self.relation.root_page,
            start,
            end,
            self.guard.take(),
            self.pager,
        )?))
    }
}

fn tighten_lower_bound<T: Ord>(current: &mut Bound<T>, new: Bound<T>) {
    let replace = match (&*current, &new) {
        (Bound::Unbounded, _) => true,

        (Bound::Included(a), Bound::Included(b)) => b > a,

        (Bound::Included(a), Bound::Excluded(b)) => b >= a,

        (Bound::Excluded(a), Bound::Included(b)) => b > a,

        (Bound::Excluded(a), Bound::Excluded(b)) => b > a,
        _ => unreachable!(),
    };

    if replace {
        *current = new;
    }
}

fn tighten_upper_bound<T: Ord>(current: &mut Bound<T>, new: Bound<T>) {
    let replace = match (&*current, &new) {
        (Bound::Unbounded, _) => true,

        (Bound::Included(a), Bound::Included(b)) => b < a,

        (Bound::Included(a), Bound::Excluded(b)) => b <= a,

        (Bound::Excluded(a), Bound::Included(b)) => b < a,

        (Bound::Excluded(a), Bound::Excluded(b)) => b < a,
        _ => unreachable!(),
    };

    if replace {
        *current = new;
    }
}

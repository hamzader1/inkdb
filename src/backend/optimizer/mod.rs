use std::ops::Bound;

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

pub struct Optimizer<'a, F: SqliteFile> {
    sqlite_master: &'a SqliteMaster,
    relation: &'a crate::schema::Table,
    plan: &'a mut Plan<F>,
    pager: &'a mut Pager<F>,
    arena: &'a ExprArena,
    // if len == requested_len we have a ready index
    ready_index: Option<Plan<F>>,
    // The scan guard factory is spent exactly once, up front. A second
    // index lookaside in the same predicate (a = x AND b = y) must reuse
    // the first scan, never build a second one: the factory is FnOnce
    // and a second take panics.
    scan_guard: Option<Box<dyn ScanGuard<F>>>,
    is_done: bool,
}

impl<'a, F: SqliteFile> Optimizer<'a, F> {
    pub fn new<G>(
        plan: &'a mut Plan<F>,
        pager: &'a mut Pager<F>,
        sqlite_master: &'a SqliteMaster,
        table_name: &str,
        root_page: u32,
        arena: &'a ExprArena,
        // Allows the optimizer to modify the source plans to be either SafeScan or Unsafe.
        mut guard: CustomScanGuard<G, F>,
    ) -> Self
    where
        G: FnOnce() -> Box<dyn ScanGuard<F>>,
    {
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
            scan_guard: Some(guard.take()),
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
                        None => self.try_index(right, left, flip_comparison(op)),
                    };
                }
            },
            Expr::Or { .. } => {
                let _ = self.ready_index.take(); // burn the index
                self.is_done = true;
                return Ok(());
            }
            Expr::And { .. } => {
                // Exact matches beat ranges: gather every conjunct, probe
                // the equality leaves first so a range never spends the
                // single guard before an exact on the same index is seen.
                // Either way the kept Filter verifies the full predicate.
                let mut leaves = Vec::new();
                Self::collect_conjuncts(self.arena, predict, &mut leaves);
                let mut exact_built = false;
                for &leaf in &leaves {
                    if self.try_exact_side(leaf)? {
                        exact_built = true;
                        break;
                    }
                }
                if !exact_built {
                    for &leaf in &leaves {
                        self.optimaze_where(leaf)?;
                        if self.is_done {
                            break;
                        }
                    }
                }
            }
            // Anything else (bare columns, literals, arithmetic) has no
            // index shape.
            _ => {}
        };
        Ok(())
    }

    /// Flatten a chain of ANDs into its conjunct leaves. Anything that
    /// is not itself an AND (equalities, ranges, ORs, bare nodes) stays
    /// whole for the normal per leaf handling.
    fn collect_conjuncts(arena: &ExprArena, node: usize, out: &mut Vec<usize>) {
        if let Expr::And { left, right } = arena.nodes[node] {
            Self::collect_conjuncts(arena, left, out);
            Self::collect_conjuncts(arena, right, out);
        } else {
            out.push(node);
        }
    }

    /// True when the leaf is an equality with a usable index, building
    /// the exact scan as a side effect. Both operand orders tried.
    fn try_exact_side(&mut self, node: usize) -> SqliteResult<bool> {
        if let Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } = self.arena.nodes[node]
        {
            if self.try_index(left, right, BinaryOperator::Eq)?.is_some() {
                return Ok(true);
            }
            if self.try_index(right, left, BinaryOperator::Eq)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
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
                if let Some(Plan::IndexRangeScan(irc)) = self.ready_index.as_mut()
                    && irc.index_root_page() == index_root_page
                {
                    match op {
                        BinaryOperator::Ge => {
                            tighten_lower_bound(&mut irc.range.0, Bound::Included(target));
                            return Ok(None);
                        }
                        BinaryOperator::Gt => {
                            tighten_lower_bound(&mut irc.range.0, Bound::Excluded(target));
                            return Ok(None);
                        }
                        BinaryOperator::Le => {
                            tighten_upper_bound(&mut irc.range.1, Bound::Included(target));
                            return Ok(None);
                        }
                        BinaryOperator::Lt => {
                            tighten_upper_bound(&mut irc.range.1, Bound::Excluded(target));
                            return Ok(None);
                        }
                        // Exact replaces the range below, keep going.
                        _ => {}
                    }
                }
                // One scan per predicate: a second lookaside keeps the
                // first scan and lets the kept Filter verify the rest.
                let Some(scan_guard) = self.scan_guard.take() else {
                    return Ok(None);
                };
                let index_plan = match op {
                    BinaryOperator::Eq => {
                        self.new_index_exact_match(index_root_page, target, scan_guard)?
                    }

                    BinaryOperator::Ge => self.new_index_range_scan(
                        index_root_page,
                        Bound::Included(target),
                        Bound::Unbounded,
                        scan_guard,
                    )?,

                    BinaryOperator::Gt => self.new_index_range_scan(
                        index_root_page,
                        Bound::Excluded(target),
                        Bound::Unbounded,
                        scan_guard,
                    )?,

                    BinaryOperator::Le => self.new_index_range_scan(
                        index_root_page,
                        Bound::Unbounded,
                        Bound::Included(target),
                        scan_guard,
                    )?,

                    BinaryOperator::Lt => self.new_index_range_scan(
                        index_root_page,
                        Bound::Unbounded,
                        Bound::Excluded(target),
                        scan_guard,
                    )?,
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
        scan_guard: Box<dyn ScanGuard<F>>,
    ) -> SqliteResult<Plan<F>> {
        Ok(Plan::IndexExactMatch(IndexExactMatch::new(
            self.pager,
            index_root_page,
            self.relation.root_page,
            target,
            scan_guard,
        )?))
    }
    pub fn new_index_range_scan(
        &mut self,
        index_root_page: u32,
        start: Bound<Value<'static>>,
        end: Bound<Value<'static>>,
        scan_guard: Box<dyn ScanGuard<F>>,
    ) -> SqliteResult<Plan<F>> {
        Ok(Plan::IndexRangeScan(IndexRangeScan::new(
            index_root_page,
            self.relation.root_page,
            start,
            end,
            scan_guard,
            self.pager,
        )?))
    }
}

fn flip_comparison(op: BinaryOperator) -> BinaryOperator {
    match op {
        BinaryOperator::Eq => BinaryOperator::Eq,
        BinaryOperator::Gt => BinaryOperator::Lt,
        BinaryOperator::Lt => BinaryOperator::Gt,
        BinaryOperator::Ge => BinaryOperator::Le,
        BinaryOperator::Le => BinaryOperator::Ge,
        BinaryOperator::NotEq => BinaryOperator::NotEq,
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

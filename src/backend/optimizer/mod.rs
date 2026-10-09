use std::ops::Bound;

use super::executor::index::IndexRangeScan;
use super::executor::scan_guard::{ScanGuard, ScanMode};
use super::planner::plan::Plan;
use crate::backend::executor::eval::Eval;
use crate::backend::executor::index::IndexExactMatch;
use crate::backend::executor::rowid::RowRangeScan;
use crate::record::Value;
use crate::sql::ast::{BinaryOperator, Expr};
use crate::sql::parser::ExprArena;
use crate::vfs::Vfs;
use crate::{InkResult, Master};

/// Try to replace the scan under a filter with an index or rowid scan.
///
/// The filter stays in place, and only the scan beneath it is replaced.
/// Even if the new scan returns too many or too few rows, the filter above
/// still checks the full predicate, ensuring the result is correct.
pub(crate) fn optimize_index_scan<V: Vfs>(
    plan: &mut Plan<V>,
    master: &Master,
    table_name: &str,
    arena: &ExprArena,
    mode: ScanMode,
) -> InkResult<()> {
    let Plan::Filter(_) = plan else {
        return Ok(());
    };
    let Some(relation) = master.table(table_name) else {
        return Ok(());
    };
    let mut optimizer = Optimizer {
        master,
        relation,
        plan,
        arena,
        ready_index: None,
        mode,
        guard_spent: false,
        is_done: false,
    };
    optimizer.optimize()
}

/// Works out whether a filter can be answered by an index scan instead.
///
/// At most one index is used per filter, and once one is chosen the rest of the
/// predicate is left to the filter. The guard is spent when the chosen scan is
/// built, and only one scan gets one, since a guard follows a single cursor.
struct Optimizer<'a, V: Vfs> {
    master: &'a Master,
    relation: &'a crate::schema::Table,
    plan: &'a mut Plan<V>,
    arena: &'a ExprArena,
    ready_index: Option<Plan<V>>,
    mode: ScanMode,
    guard_spent: bool,
    is_done: bool,
}

impl<'a, V: Vfs> Optimizer<'a, V> {
    /// The guard for the chosen scan, or nothing if one was already spent.
    fn take_guard(&mut self) -> Option<Box<dyn ScanGuard<V>>> {
        if self.guard_spent {
            return None;
        }
        self.guard_spent = true;
        Some(self.mode.guard())
    }

    /// Look at the predicate and, if an index scan comes out of it, put it
    /// under the filter.
    fn optimize(&mut self) -> InkResult<()> {
        let Plan::Filter(filter) = self.plan else {
            return Ok(());
        };

        let predicate = filter.predicate();
        self.optimize_where(predicate)?;

        if self.is_done {
            return Ok(());
        }
        let Some(new_plan) = self.ready_index.take() else {
            return Ok(());
        };
        match new_plan {
            Plan::IndexExactMatch(_) | Plan::IndexRangeScan(_) | Plan::RowRangeScan(_) => {
                if let Plan::Filter(filter) = self.plan {
                    *filter.child_mut() = new_plan;
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Look for an index shape in a predicate.
    ///
    /// A comparison between a column and a constant is the shape that counts, and
    /// either side may be the column. An AND is flattened so every part gets a
    /// look, equality first, while an OR refuses the whole predicate since no
    /// single index covers both of its sides.
    fn optimize_where(&mut self, predicate: usize) -> InkResult<()> {
        if self.is_done {
            return Ok(());
        }
        match self.arena.nodes[predicate] {
            Expr::BinaryOp { left, op, right } => match op {
                BinaryOperator::NotEq | BinaryOperator::Is | BinaryOperator::IsNot => return Ok(()),
                _ => {
                    match self.try_index(left, right, op)? {
                        Some(_) => return Ok(()),
                        None => self.try_index(right, left, flip_comparison(op))?,
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
                Self::collect_conjuncts(self.arena, predicate, &mut leaves);
                let mut exact_built = false;
                for &leaf in &leaves {
                    if self.try_exact_side(leaf)? {
                        exact_built = true;
                        break;
                    }
                }
                if !exact_built {
                    for &leaf in &leaves {
                        self.optimize_where(leaf)?;
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
    /// Flatten a chain of ANDs into the leaves it is made of, so each can be
    /// tried on its own.
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
    /// Try an equality leaf against an index, in both operand orders.
    fn try_exact_side(&mut self, node: usize) -> InkResult<bool> {
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

    /// Try to turn one comparison into a scan.
    ///
    /// A range on the row id column becomes a walk over the table tree, and a
    /// comparison against an indexed column becomes an entry lookup or an index
    /// range. When a range is already there on the same index, another bound
    /// narrows it instead of replacing it; an equality replaces it outright.
    fn try_index(
        &mut self,
        left: usize,
        right: usize,
        op: BinaryOperator,
    ) -> InkResult<Option<()>> {
        if let Expr::ColumnRef(i) = self.arena.nodes[left]
        // && let BinaryOperator::Eq = op
        {
            // Right must be const
            let target = match Self::try_cast_to_const_expr(self.arena, right) {
                Some(value) => value,
                _ => return Ok(None),
            };
            let col = self.relation.get_col_name(i).unwrap();

            if self.relation.rowid_column() == Some(i)
                && let Value::Integer(rowid) = target
            {
                let bounds = match op {
                    BinaryOperator::Eq => (Bound::Included(rowid), Bound::Included(rowid)),
                    BinaryOperator::Ge => (Bound::Included(rowid), Bound::Unbounded),
                    BinaryOperator::Gt => (Bound::Excluded(rowid), Bound::Unbounded),
                    BinaryOperator::Le => (Bound::Unbounded, Bound::Included(rowid)),
                    BinaryOperator::Lt => (Bound::Unbounded, Bound::Excluded(rowid)),
                    _ => return Ok(None),
                };
                let root_page = self.relation.root_page();
                if let Some(Plan::RowRangeScan(rrs)) = self.ready_index.as_mut()
                    && rrs.root_page() == root_page
                {
                    match op {
                        BinaryOperator::Eq => {
                            rrs.range = (Bound::Included(rowid), Bound::Included(rowid));
                        }
                        BinaryOperator::Ge => {
                            tighten_lower_bound(&mut rrs.range.0, Bound::Included(rowid));
                        }
                        BinaryOperator::Gt => {
                            tighten_lower_bound(&mut rrs.range.0, Bound::Excluded(rowid));
                        }
                        BinaryOperator::Le => {
                            tighten_upper_bound(&mut rrs.range.1, Bound::Included(rowid));
                        }
                        BinaryOperator::Lt => {
                            tighten_upper_bound(&mut rrs.range.1, Bound::Excluded(rowid));
                        }
                        _ => {}
                    }
                    return Ok(Some(()));
                }
                let Some(scan_guard) = self.take_guard() else {
                    return Ok(None);
                };
                self.ready_index = Some(Plan::RowRangeScan(RowRangeScan::new(
                    root_page,
                    self.relation.rowid_column(),
                    bounds.0,
                    bounds.1,
                    scan_guard,
                )));
                return Ok(Some(()));
            }

            let mut index_root_page = None;
            for index in self.master.indexes().values() {
                if index.is_on(&col.name, self.relation.name()) {
                    index_root_page = Some(index.root_page());
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
                        _ => {}
                    }
                }
                let Some(scan_guard) = self.take_guard() else {
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
    /// Evaluate an expression with no row behind it, which is how the constant
    /// side of a comparison is found.
    fn try_cast_to_const_expr(arena: &ExprArena, index: usize) -> Option<Value<'static>> {
        Eval::eval(arena, index, None).ok()
    }

    /// Build the exact match scan.
    fn new_index_exact_match(
        &mut self,
        index_root_page: u32,
        target: Value<'static>,
        scan_guard: Box<dyn ScanGuard<V>>,
    ) -> InkResult<Plan<V>> {
        Ok(Plan::IndexExactMatch(IndexExactMatch::new(
            index_root_page,
            self.relation.root_page(),
            self.relation.name().clone(),
            target,
            scan_guard,
        )?))
    }
    /// Build the index range scan.
    fn new_index_range_scan(
        &mut self,
        index_root_page: u32,
        start: Bound<Value<'static>>,
        end: Bound<Value<'static>>,
        scan_guard: Box<dyn ScanGuard<V>>,
    ) -> InkResult<Plan<V>> {
        Ok(Plan::IndexRangeScan(IndexRangeScan::new(
            index_root_page,
            self.relation.root_page(),
            start,
            end,
            scan_guard,
        )?))
    }
}

/// Turn `a > b` into `b < a`, for when the column is on the right and the
/// constant on the left.
fn flip_comparison(op: BinaryOperator) -> BinaryOperator {
    match op {
        BinaryOperator::Eq => BinaryOperator::Eq,
        BinaryOperator::Gt => BinaryOperator::Lt,
        BinaryOperator::Lt => BinaryOperator::Gt,
        BinaryOperator::Ge => BinaryOperator::Le,
        BinaryOperator::Le => BinaryOperator::Ge,
        BinaryOperator::NotEq => BinaryOperator::NotEq,
        BinaryOperator::Is => BinaryOperator::Is,
        BinaryOperator::IsNot => BinaryOperator::IsNot,
    }
}

/// Move a lower bound in towards the middle, keeping whichever bound is
/// tighter. An included bound is looser than an excluded one at the same value.
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

/// The same for an upper bound.
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

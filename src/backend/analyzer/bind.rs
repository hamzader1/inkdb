use crate::errors::InkError;
use crate::schema::TableSchema;
use crate::sql::ast::Expr;
use crate::sql::parser::ExprArena;

use super::Analyze;

impl<'a> Analyze<'a> {
    /// Bind every name in an expression, writing a brand new one.
    ///
    /// This is used when the expression list changes shape, which happens for a
    /// SELECT with a star: the star stands for however many columns the table has,
    /// so those columns are spliced into the list and every other expression is
    /// moved along. `map` records where each old node ended up, `new_cols`
    /// collects the output columns, and both are filled as the expression is
    /// walked.
    /// # Example
    /// Let's say we want to run this query:
    /// ```sql
    /// SELECT *, salary * 2 FROM employees;
    /// ```
    /// The table has the columns `["name", "age", "salary", "position"]`.
    ///
    /// The arena initially contains the following nodes:
    /// `["*", Ident("salary"), Int(2), Mul(1, 2)]`.
    ///
    /// If we expand `*` in place, we get:
    /// `["Ident(name)", "Ident(age)", "Ident(salary)", "Ident(position)",
    /// Ident("salary"), Int(2), Mul(1, 2)]`.
    ///
    /// This produces an incorrect result because the `Mul` node still points to
    /// nodes 1 and 2, which are now `Ident(age)` and `Ident(salary)`.
    ///
    /// To fix this, we create a new arena and a mapper to update the node pointers
    /// as we expand the columns.
    ///
    /// The rule is simple: whenever we encounter `*`, we expand it into the
    /// columns stored in `new_cols` and record the new index in the mapper.
    ///
    /// Initially, both the new arena and the mapper are empty. We start walking
    /// through the original arena.
    ///
    /// The first node is `*`. We expand it into the four columns and record the
    /// index of the last expanded column in the mapper. Since there are four
    /// columns, the last index is `4 - 1 = 3`.
    ///
    /// The mapper becomes `[3]`.
    ///
    /// Next, we encounter `Ident("salary")`. We add it to the new arena and record
    /// its index in the mapper, giving us `[3, 4]`.
    ///
    /// Next, we encounter `Int(2)`. We add it to the new arena and update the
    /// mapper to `[3, 4, 5]`.
    ///
    /// Finally, we reach `Mul`, whose pointers are now incorrect. To fix them, we
    /// call [`Expr::remap_l_r`] with the indexes from the mapper.
    ///
    /// The original `Mul` node points to indexes 1 and 2. We look up those indexes
    /// in the mapper:
    /// * `map[1] = 4`
    /// * `map[2] = 5`
    ///
    /// The updated node becomes `Mul(4, 5)`.
    ///
    /// The new arena is now correct:
    /// `["Ident(name)", "Ident(age)", "Ident(salary)", "Ident(position)",
    /// Ident("salary"), Int(2), Mul(4, 5)]`.
    ///
    pub(super) fn slow_bind(
        table: &impl TableSchema,
        idx: usize,
        arena: &mut ExprArena,
        // new to move
        new: &mut Vec<Expr>,
        map: &mut Vec<usize>,
        new_cols: &mut Vec<usize>,
    ) -> Result<(), InkError> {
        let mut sink = SlowBind { new, map, new_cols };
        Self::walk(table, idx, arena, &mut sink)?;
        Ok(())
    }

    // General purpose
    /// Bind every name in an expression in place.
    ///
    /// Each identifier becomes a column reference and nothing else moves, so the
    /// arena keeps its shape and the expression index stays valid. This is what a
    /// WHERE clause, a LIMIT or an UPDATE target uses.
    pub(super) fn fast_bind(
        table: &impl TableSchema,
        idx: usize,
        arena: &mut ExprArena,
    ) -> Result<(), InkError> {
        let mut sink = FastBind;
        let pos = Self::walk(table, idx, arena, &mut sink)?;
        debug_assert_eq!(pos, idx, "fast_bind must bind in place");
        Ok(())
    }
}

impl<'a> Analyze<'a> {
    /// Walk an expression and hand each node to a sink.
    ///
    /// The sink decides what each node becomes and what index it takes, which is
    /// the whole difference between binding in place and binding into a new arena.
    /// The answer is the index the node came out at.
    pub(crate) fn walk(
        table: &impl TableSchema,
        idx: usize,
        arena: &mut ExprArena,
        sink: &mut impl BindSink,
    ) -> Result<usize, InkError> {
        match arena.nodes[idx].clone() {
            Expr::Identifier(name) => sink.ident(table, arena, idx, &name),
            Expr::Star => sink.star(table, arena, idx),
            node @ Expr::Null => Ok(sink.leaf(arena, node, idx)),
            node @ (Expr::Number(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::StringLitteral(_)
            | Expr::Blob(_)) => Ok(sink.leaf(arena, node, idx)),
            Expr::Add(l, r) | Expr::Substract(l, r) | Expr::Multiply(l, r) | Expr::Devide(l, r) => {
                let node = arena.nodes[idx].clone();
                let bound_l = Self::walk(table, l, arena, sink)?;
                let bound_r = Self::walk(table, r, arena, sink)?;
                Ok(sink.binary(&node, idx, bound_l, bound_r))
            }
            Expr::Neg(child) | Expr::Not(child) => {
                let node = arena.nodes[idx].clone();
                let bound = Self::walk(table, child, arena, sink)?;
                Ok(sink.unary(arena, node, idx, bound))
            }
            Expr::And { left, right }
            | Expr::Or { left, right }
            | Expr::BinaryOp { left, right, .. } => {
                let node = arena.nodes[idx].clone();
                let bound_l = Self::walk(table, left, arena, sink)?;
                let bound_r = Self::walk(table, right, arena, sink)?;
                Ok(sink.binary(&node, idx, bound_l, bound_r))
            }
            other => Err(sink.unsupported(&other)),
        }
    }
}

/// The sink that binds into a new arena, remembering where each node came out.
///
/// An expression list is rebuilt whenever a star has to be expanded, and the new
/// arena is longer than the old one, so the executor cannot use the old indices.
/// `map` is the translation from the old arena to the new one, and `new_cols` is
/// the output column list as it is rebuilt.
pub(crate) struct SlowBind<'a> {
    new: &'a mut Vec<Expr>,
    map: &'a mut Vec<usize>,
    new_cols: &'a mut Vec<usize>,
}

/// What gets done with each node while an expression is bound.
///
/// One implementation binds in place and another builds a new arena, and the walk
/// itself does not care which, so the two share one traversal.
pub(crate) trait BindSink {
    /// Bind an identifier to a column.
    fn ident(
        &mut self,
        table: &impl TableSchema,
        arena: &mut ExprArena,
        idx: usize,
        name: &str,
    ) -> Result<usize, InkError>;
    /// Take a literal or a constant as it is.
    fn leaf(&mut self, arena: &mut ExprArena, expr: Expr, idx: usize) -> usize;
    /// Expand a star into every column of the table.
    fn star(
        &mut self,
        table: &impl TableSchema,
        arena: &mut ExprArena,
        idx: usize,
    ) -> Result<usize, InkError>;
    /// Rebuild a unary expression around its bound child.
    fn unary(&mut self, arena: &mut ExprArena, node: Expr, idx: usize, child: usize) -> usize;
    /// Rebuild a binary expression around its two bound children.
    fn binary(&mut self, node: &Expr, idx: usize, l: usize, r: usize) -> usize;
    /// The error for an expression this sink cannot bind.
    fn unsupported(&mut self, expr: &Expr) -> InkError;
}
impl BindSink for SlowBind<'_> {
    fn ident(
        &mut self,
        table: &impl TableSchema,
        _arena: &mut ExprArena,
        idx: usize,
        name: &str,
    ) -> Result<usize, InkError> {
        match table.column_index(&name.to_lowercase()) {
            Some(col_idx) => {
                self.new.push(Expr::ColumnRef(col_idx));
                self.map[idx] = self.new.len() - 1;
                Ok(self.new.len() - 1)
            }
            _ => Err(InkError::UnknownColumn(name.into())),
        }
    }
    fn leaf(&mut self, _arena: &mut ExprArena, expr: Expr, idx: usize) -> usize {
        self.new.push(expr);
        let return_index = self.new.len() - 1;
        self.map[idx] = return_index;
        return_index
    }
    fn star(
        &mut self,
        table: &impl TableSchema,
        _arena: &mut ExprArena,
        idx: usize,
    ) -> Result<usize, InkError> {
        for i in 0..table.columns_len() {
            self.new.push(Expr::ColumnRef(i));
            self.new_cols.push(self.new.len() - 1);
        }
        self.map[idx] = self.new.len() - 1;
        Ok(self.new.len() - 1)
    }
    fn unary(&mut self, _arena: &mut ExprArena, node: Expr, idx: usize, child: usize) -> usize {
        match node {
            Expr::Neg(_) => {
                self.new.push(Expr::Neg(child)); // the return from the recursive call
                self.map[idx] = self.new.len() - 1;
                self.new.len() - 1
            }
            Expr::Not(_) => {
                self.new.push(Expr::Not(child)); // the return from the recursive call
                self.map[idx] = self.new.len() - 1;
                self.new.len() - 1
            }
            _ => unreachable!("called unary on non-unary expression"),
        }
    }
    fn binary(&mut self, node: &Expr, idx: usize, l: usize, r: usize) -> usize {
        match node {
            Expr::Add(_, _) | Expr::Substract(_, _) | Expr::Devide(_, _) | Expr::Multiply(_, _) => {
                self.new.push(Expr::remap_l_r(node, l, r));
                self.map[idx] = self.new.len() - 1;
                self.new.len() - 1
            }
            Expr::And { .. } | Expr::Or { .. } | Expr::BinaryOp { .. } => {
                self.new.push(Expr::remap_l_r(node, l, r));
                self.map[idx] = self.new.len() - 1;
                self.new.len() - 1
            }
            _ => unreachable!("Called binary on non binary expression"),
        }
    }
    fn unsupported(&mut self, expr: &Expr) -> InkError {
        InkError::runtime(format!(
            "Expression '{expr}' cannot appear in a SELECT column list (only columns, '*' and arithmetic/comparison expressions are supported)"
        ))
    }
}

/// The sink that binds an expression in place.
pub(crate) struct FastBind;
impl BindSink for FastBind {
    fn ident(
        &mut self,
        table: &impl TableSchema,
        arena: &mut ExprArena,
        idx: usize,
        name: &str,
    ) -> Result<usize, InkError> {
        match table.column_index(&name.to_lowercase()) {
            Some(col_idx) => {
                arena.nodes[idx] = Expr::ColumnRef(col_idx);
                Ok(idx)
            }
            _ => Err(InkError::UnknownColumn(name.into())),
        }
    }
    fn leaf(&mut self, _arena: &mut ExprArena, _expr: Expr, idx: usize) -> usize {
        idx
    }
    fn star(
        &mut self,
        _table: &impl TableSchema,
        _arena: &mut ExprArena,
        _idx: usize,
    ) -> Result<usize, InkError> {
        Err(InkError::runtime(
            "Expression * cannot appear in WHERE/LIMIT (only columns and arithmetic/comparison expressions are supported",
        ))
    }
    fn unary(&mut self, _arena: &mut ExprArena, _node: Expr, idx: usize, _child: usize) -> usize {
        idx
    }
    fn binary(&mut self, _node: &Expr, idx: usize, _l: usize, _r: usize) -> usize {
        idx
    }
    fn unsupported(&mut self, expr: &Expr) -> InkError {
        InkError::runtime(format!(
            "Expression '{expr}' cannot appear in WHERE/LIMIT (only columns and arithmetic/comparison expressions are supported)"
        ))
    }
}

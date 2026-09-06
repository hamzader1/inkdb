use crate::errors::SqliteError;
use crate::schema::Table;
use crate::sql::ast::Expr;
use crate::sql::parser::ExprArena;

use super::Analyze;

impl Analyze {
    pub(super) fn slow_bind(
        table: &Table,
        idx: usize,
        arena: &mut ExprArena,
        // new to move
        new: &mut Vec<Expr>,
        map: &mut Vec<usize>,
        new_cols: &mut Vec<usize>,
    ) -> Result<(), SqliteError> {
        let mut sink = SlowBind { new, map, new_cols };
        Self::walk(table, idx, arena, &mut sink)?;
        Ok(())
    }

    // General purpose
    pub(super) fn fast_bind(
        table: &Table,
        idx: usize,
        arena: &mut ExprArena,
    ) -> Result<(), SqliteError> {
        let mut sink = FastBind;
        let pos = Self::walk(table, idx, arena, &mut sink)?;
        debug_assert_eq!(pos, idx, "fast_bind must bind in place");
        Ok(())
    }
}

/// Single post-order traversal shared by both binders. Each hook returns
/// "where this node ended up in the sink's world": a position in the new
/// arena for slow (things move, `*` expands), the input index for fast
/// (binding happens in place).
impl Analyze {
    pub fn walk(
        table: &Table,
        idx: usize,
        arena: &mut ExprArena,
        sink: &mut impl BindSink,
    ) -> Result<usize, SqliteError> {
        match arena.nodes[idx].clone() {
            Expr::Identifier(name) => sink.ident(table, arena, idx, &name),
            Expr::Star => sink.star(table, arena, idx),
            node @ (Expr::Number(_) | Expr::Float(_) | Expr::Bool(_) | Expr::StringLitteral(_)) => {
                Ok(sink.leaf(arena, node, idx))
            }
            Expr::Add(l, r) | Expr::Substract(l, r) | Expr::Multiply(l, r) | Expr::Devide(l, r) => {
                // Cloned again so the hook can see which operator this is; the
                // dispatch clone above only carried the children out.
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

pub struct SlowBind<'a> {
    new: &'a mut Vec<Expr>,
    map: &'a mut Vec<usize>,
    new_cols: &'a mut Vec<usize>,
}

pub trait BindSink {
    // Each hook returns "where this node ended up in MY world".
    fn ident(
        &mut self,
        table: &Table,
        arena: &mut ExprArena,
        idx: usize,
        name: &str,
    ) -> Result<usize, SqliteError>;
    fn leaf(&mut self, arena: &mut ExprArena, expr: Expr, idx: usize) -> usize;
    fn star(
        &mut self,
        table: &Table,
        arena: &mut ExprArena,
        idx: usize,
    ) -> Result<usize, SqliteError>;
    fn unary(&mut self, arena: &mut ExprArena, node: Expr, idx: usize, child: usize) -> usize;
    fn binary(&mut self, node: &Expr, idx: usize, l: usize, r: usize) -> usize;
    /// Error for node kinds this binder rejects (`Star` in WHERE/LIMIT,
    /// already-bound `ColumnRef`, ...). Keeps sink-specific messages out
    /// of the shared walk.
    fn unsupported(&mut self, expr: &Expr) -> SqliteError;
}
impl BindSink for SlowBind<'_> {
    fn ident(
        &mut self,
        table: &Table,
        arena: &mut ExprArena,
        idx: usize,
        name: &str,
    ) -> Result<usize, SqliteError> {
        match table.get_col_idx(&name.to_lowercase()) {
            Some(col_idx) => {
                self.new.push(Expr::ColumnRef(col_idx));
                self.map[idx] = self.new.len() - 1;
                Ok(self.new.len() - 1)
            }
            _ => Err(SqliteError::UnknownColumn(name.into())),
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
        table: &Table,
        _arena: &mut ExprArena,
        idx: usize,
    ) -> Result<usize, SqliteError> {
        for i in 0..table.get_cols_len() {
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
    fn unsupported(&mut self, expr: &Expr) -> SqliteError {
        SqliteError::Runtime(format!(
            "Expression '{expr}' cannot appear in a SELECT column list (only columns, '*' and arithmetic/comparison expressions are supported)"
        ))
    }
}

/*
 *
 * that too much
 *
*/

pub struct FastBind;
impl BindSink for FastBind {
    fn ident(
        &mut self,
        table: &Table,
        arena: &mut ExprArena,
        idx: usize,
        name: &str,
    ) -> Result<usize, SqliteError> {
        match table.get_col_idx(&name.to_lowercase()) {
            Some(col_idx) => {
                arena.nodes[idx] = Expr::ColumnRef(col_idx);
                Ok(idx)
            }
            _ => Err(SqliteError::UnknownColumn(name.into())),
        }
    }
    fn leaf(&mut self, _arena: &mut ExprArena, _expr: Expr, idx: usize) -> usize {
        idx
    }
    fn star(
        &mut self,
        table: &Table,
        arena: &mut ExprArena,
        idx: usize,
    ) -> Result<usize, SqliteError> {
        Err(SqliteError::Runtime(
            "Expression * cannot appear in WHERE/LIMIT (only columns and arithmetic/comparison expressions are supported".into(),
        ))
    }
    fn unary(&mut self, _arena: &mut ExprArena, _node: Expr, idx: usize, _child: usize) -> usize {
        idx
    }
    fn binary(&mut self, _node: &Expr, idx: usize, _l: usize, _r: usize) -> usize {
        idx
    }
    fn unsupported(&mut self, expr: &Expr) -> SqliteError {
        SqliteError::Runtime(format!(
            "Expression '{expr}' cannot appear in WHERE/LIMIT (only columns and arithmetic/comparison expressions are supported)"
        ))
    }
}

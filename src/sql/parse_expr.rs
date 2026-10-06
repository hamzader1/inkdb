use super::ast::{BinaryOperator, Expr};
use super::parser::Parser;
use super::tokens::TokenKind::*;
use crate::errors::InkError;

/// Expressions, from the loosest operator down to the tightest.
///
/// Each function here handles one level of precedence and leans on the one below
/// it, so the order the functions are written in is the order the operators bind.
/// `OR` is read first and so binds loosest, then `AND`, then comparisons, then
/// `+` and `-`, then `*` and `/`, and finally a single value or a bracketed
/// expression. Everything built along the way goes into the arena, and what
/// comes back is the index of the finished expression.
///
/// # Example
/// We try to parse the following expression:
/// WHERE
/// (a + b * c > 10 AND d / e < 5) OR f - g = 2
/// The tree would be expressed as
/**
               OR
              /  \
            AND   =
           /   \ / \
          >    < -  2
         / \  / \ / \
        +  10 /  5 f  g
       / \   / \
      a   * d   e
         / \
        b   c
*/
/// For simpler one:
/// a + b * c > 10 AND d = 5
/// the arena should look like:
/**
 [
     a,                 // 0
     b,                 // 1
     c,                 // 2
     Mul(1, 2),         // 3  => b * c
     Add(0, 3),         // 4  => a + (b * c)
     10,                // 5
     GreaterThan(4, 5), // 6  => a + b*c > 10
     d,                 // 7
     5,                 // 8
     Equal(7, 8),       // 9  => d = 5
     And(6, 9),         // 10 => (a+b*c > 10) AND (d=5)
 ]
*/
impl Parser {
    /// Read a whole expression, which is the way in for callers.
    pub(crate) fn parse_expression(&mut self) -> Result<usize, InkError> {
        self.parse_logical_or()
    }

    /// Read an `OR` chain. The left side is read first and every following `OR`
    /// adds another level, which keeps `a OR b OR c` grouping to the left.
    pub(crate) fn parse_logical_or(&mut self) -> Result<usize, InkError> {
        let mut left = self.parse_logical_and()?;

        while self.eat(Or) {
            let right = self.parse_logical_and()?;

            left = self.arena.push(Expr::Or { left, right });
        }
        Ok(left)
    }

    /// Read an `AND` chain, one level tighter than `OR`.
    pub(crate) fn parse_logical_and(&mut self) -> Result<usize, InkError> {
        let mut left = self.parse_condition()?;
        while self.eat(And) {
            let right = self.parse_condition()?;
            left = self.arena.push(Expr::And { left, right });
        }
        Ok(left)
    }

    /// Read a comparison. Any of `=`, `!=`, `>=`, `>`, `<=` and `<` are handled
    /// here, and so is `IS NULL` and `IS NOT NULL`.
    pub(crate) fn parse_condition(&mut self) -> Result<usize, InkError> {
        let mut left = self.parse_addition()?;
        while self.at(Equals)
            || self.at(NotEquals)
            || self.at(Ge)
            || self.at(Gt)
            || self.at(Le)
            || self.at(Lt)
            || self.at(Is)
        {
            if self.eat(Is) {
                let not = self.eat(Not);
                self.expect(Null)?;
                let right = self.arena.push(Expr::Null);
                let op = if not {
                    BinaryOperator::IsNot
                } else {
                    BinaryOperator::Is
                };
                left = self.arena.push(Expr::BinaryOp { left, op, right });
                continue;
            }
            let op = if self.eat(Equals) {
                BinaryOperator::Eq
            } else if self.eat(NotEquals) {
                BinaryOperator::NotEq
            } else if self.eat(Ge) {
                BinaryOperator::Ge
            } else if self.eat(Gt) {
                BinaryOperator::Gt
            } else if self.eat(Le) {
                BinaryOperator::Le
            } else {
                self.eat(Lt);
                BinaryOperator::Lt
            };
            let right = self.parse_addition()?;
            left = self.arena.push(Expr::BinaryOp { left, op, right })
        }
        Ok(left)
    }
    /// Read a `+` or `-` chain, which binds tighter than a comparison.
    pub(crate) fn parse_addition(&mut self) -> Result<usize, InkError> {
        let mut left = self.parse_multiplication()?;
        while self.at(Plus) || self.at(Minus) {
            if self.eat(Plus) {
                let right = self.parse_multiplication()?;
                left = self.arena.push(Expr::Add(left, right));
            } else {
                self.eat(Minus);
                let right = self.parse_multiplication()?;
                left = self.arena.push(Expr::Substract(left, right));
            }
        }
        Ok(left)
    }
    /// Read a `*` or `/` chain, the tightest of the binary operators.
    pub(crate) fn parse_multiplication(&mut self) -> Result<usize, InkError> {
        let mut left = self.parse_unary()?;
        while self.at(Star) || self.at(Slash) {
            if self.eat(Star) {
                let right = self.parse_unary()?;
                left = self.arena.push(Expr::Multiply(left, right));
            } else {
                self.eat(Slash);
                let right = self.parse_unary()?;
                left = self.arena.push(Expr::Devide(left, right));
            }
        }
        Ok(left)
    }
    /// Read a leading `-` or `NOT`.
    ///
    /// A minus in front of a plain number is folded into the number itself, so
    /// `-5` is one value rather than a negation of another. Anything else keeps
    /// the negation as its own expression.
    fn parse_unary(&mut self) -> Result<usize, InkError> {
        if self.eat(Minus) {
            let idx = self.parse_factor()?;

            match self.arena.nodes[idx] {
                Expr::Number(x) => Ok(self.arena.push(Expr::Number(-x))),

                Expr::Float(x) => Ok(self.arena.push(Expr::Float(-x))),

                _ => Ok(self.arena.push(Expr::Neg(idx))),
            }
        } else if self.eat(Not) {
            let idx = self.parse_condition()?;
            Ok(self.arena.push(Expr::Not(idx)))
        } else {
            self.parse_factor()
        }
    }
    /// Read a single value: a bracketed expression, a name, a function call, or
    /// one of the literals such as a string, number, blob, boolean or `NULL`.
    ///
    /// # Errors
    /// An error naming the token when it is none of those, which is what a
    /// statement with a stray operator in the middle of an expression gets.
    pub(crate) fn parse_factor(&mut self) -> Result<usize, InkError> {
        if self.eat(LeftParen) {
            let expr = self.parse_expression()?;
            self.expect(RightParen)?;
            return Ok(expr);
        }

        if let Some(Identifier(name)) = self.peek() {
            let name = name.clone();
            self.next_token();
            if self.at(LeftParen) {
                return self.parse_function(&name);
            }
            return Ok(self.arena.push(Expr::Identifier(name)));
        }

        let expr = match self.peek() {
            Some(String(x)) => self.arena.push(Expr::StringLitteral(x.clone())),
            Some(BlobVar(bytes)) => self.arena.push(Expr::Blob(bytes.clone())),
            Some(NumberVar(x)) => self.arena.push(Expr::Number(*x)),
            Some(FloatVar(x)) => self.arena.push(Expr::Float(*x)),
            Some(BoolVar(x)) => self.arena.push(Expr::Bool(*x)),
            Some(Null) => self.arena.push(Expr::Null),
            Some(other) => {
                return Err(InkError::runtime(format!(
                    "Unexpected token {:?} in expression: expected a column name, string, number, boolean or '('",
                    other
                )));
            }
            None => {
                return Err(InkError::runtime(
                    "Unexpected end of input, expected a value",
                ));
            }
        };
        self.next_token();
        Ok(expr)
    }

    /// Read a function call. Only `count` exists so far, and it takes either a
    /// single expression or a star.
    ///
    /// # Errors
    /// An error naming the function when it is not one the engine implements.
    fn parse_function(&mut self, name: &str) -> Result<usize, InkError> {
        self.expect(LeftParen)?;
        if name.eq_ignore_ascii_case("count") {
            let arg = if self.eat(Star) {
                None
            } else {
                Some(self.parse_expression()?)
            };
            self.expect(RightParen)?;
            return Ok(self.arena.push(Expr::Count { arg }));
        }
        Err(InkError::runtime(format!("unknown function: {name}")))
    }
}

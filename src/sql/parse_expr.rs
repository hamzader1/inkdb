use super::ast::{BinaryOperator, Expr};
use super::parser::Parser;
use super::tokens::TokenKind::*;
use crate::errors::InkError;

impl Parser {
    pub(crate) fn parse_expression(&mut self) -> Result<usize, InkError> {
        self.parse_logical_or()
    }

    pub(crate) fn parse_logical_or(&mut self) -> Result<usize, InkError> {
        let mut left = self.parse_logical_and()?;

        while self.eat(Or) {
            let right = self.parse_logical_and()?;

            left = self.arena.push(Expr::Or { left, right });
        }
        Ok(left)
    }

    pub(crate) fn parse_logical_and(&mut self) -> Result<usize, InkError> {
        let mut left = self.parse_condition()?;
        while self.eat(And) {
            let right = self.parse_condition()?;
            left = self.arena.push(Expr::And { left, right });
        }
        Ok(left)
    }

    pub(crate) fn parse_condition(&mut self) -> Result<usize, InkError> {
        let mut left = self.parse_addition()?;
        while self.at(Equals)
            || self.at(NotEquals)
            || self.at(Ge)
            || self.at(Gt)
            || self.at(Le)
            || self.at(Lt)
        {
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

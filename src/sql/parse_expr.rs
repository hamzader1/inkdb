use super::ast::{BinaryOperator, Expr};
use super::parser::Parser;
use super::tokens::TokenKind::*;
use crate::errors::SqliteError;

impl Parser {
    pub fn parse_expression(&mut self) -> Result<Expr, SqliteError> {
        self.parse_logical_or()
        // start
    }

    pub fn parse_logical_or(&mut self) -> Result<Expr, SqliteError> {
        let mut left = self.parse_logical_and()?;

        while self.eat(Or) {
            let right = self.parse_logical_and()?;

            left = Expr::Or {
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
        // Ok(Expr::Empty)
    }

    pub fn parse_logical_and(&mut self) -> Result<Expr, SqliteError> {
        let mut left = self.parse_condition()?;
        while self.eat(And) {
            let right = self.parse_condition()?;
            left = Expr::And {
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    pub fn parse_condition(&mut self) -> Result<Expr, SqliteError> {
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
            left = Expr::BinaryOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            }
        }
        Ok(left)
    }
    pub fn parse_addition(&mut self) -> Result<Expr, SqliteError> {
        let mut left = self.parse_multiplication()?;
        while self.at(Plus) || self.at(Minus) {
            let right = self.parse_multiplication()?;
            if self.eat(Plus) {
                left = Expr::Add(Box::new(left), Box::new(right));
            } else {
                self.eat(Minus);
                left = Expr::Substract(Box::new(left), Box::new(right));
            }
        }
        Ok(left)
    }
    pub fn parse_multiplication(&mut self) -> Result<Expr, SqliteError> {
        let mut left = self.parse_factor()?;
        while self.at(Star) || self.at(Slash) {
            let right = self.parse_factor()?;
            if self.eat(Star) {
                left = Expr::Multiply(Box::new(left), Box::new(right));
            } else {
                self.eat(Slash);
                left = Expr::Devide(Box::new(left), Box::new(right));
            }
        }
        Ok(left)
    }
    pub fn parse_factor(&mut self) -> Result<Expr, SqliteError> {
        if self.eat(LeftParen) {
            let expr = self.parse_expression()?;
            self.expect(RightParen)?;
            return Ok(expr);
        }

        let expr = match self.peek() {
            Some(Identifier(x)) => Expr::Identifier(x.clone()),
            Some(String(x)) => Expr::StringLitteral(x.clone()),
            Some(NumberVar(x)) => Expr::Number(*x),
            Some(FloatVar(x)) => Expr::Float(*x),
            Some(BoolVar(x)) => Expr::Bool(*x),
            Some(other) => {
                return Err(SqliteError::RuntimeError(format!(
                    "Unexpected token {:?} in expression",
                    other
                )));
            }
            None => {
                return Err(SqliteError::RuntimeError(
                    "Unexpected end of input, expected a value".into(),
                ));
            }
        };
        self.next();
        Ok(expr)
    }
}

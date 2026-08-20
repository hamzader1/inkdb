use super::ast::Constraint;
use super::ast::*;
use super::parser::Parser;
use super::tokens::TokenKind::{self, *};
use crate::errors::SqliteError;

impl Parser {
    pub fn parse_create(&mut self) -> Result<Ast, SqliteError> {
        self.expect(Create)?;
        let unique = self.eat(Unique);
        match self.peek() {
            Some(Table) => {
                if unique {
                    return Err(SqliteError::RuntimeError(
                        "CREATE UNIQUE TABLE is invalid".into(),
                    ));
                }
                self.parse_create_table()
            }
            Some(Index) => self.parse_create_index(unique),
            _ => Err(SqliteError::RuntimeError(
                "Expected TABLE or INDEX after CREATE".into(),
            )),
        }
    }

    fn parse_create_table(&mut self) -> Result<Ast, SqliteError> {
        self.expect(Table)?;
        let name = self.expect_ident()?.to_lowercase();
        self.expect(LeftParen)?;
        let mut columns: Vec<Column> = Vec::new();
        while !self.at(RightParen) {
            columns.push(self.parse_create_column()?);
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect(RightParen)?;
        Ok(Ast::CreateTableAst(CreateTable { name, columns }))
    }

    fn parse_create_index(&mut self, unique: bool) -> Result<Ast, SqliteError> {
        self.expect(Index)?;
        if self.eat(If) {
            self.expect(Not)?;
            self.expect(Exists)?;
        }
        let name = self.expect_ident()?;
        self.expect(On)?;
        let table = self.expect_ident()?;
        self.expect(LeftParen)?;
        let mut columns = Vec::new();
        while !self.at(RightParen) {
            columns.push(self.expect_ident()?);
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect(RightParen)?;
        Ok(Ast::CreateIndexAst(CreateIndex {
            unique,
            name,
            table,
            columns,
        }))
    }

    /// not parse_columns since [`SelectStmt`] (and Insert later) reserved it
    fn parse_create_column(&mut self) -> Result<Column, SqliteError> {
        let name = self.expect_ident()?.to_lowercase();
        let mut affinity: Option<Affinity> = None;
        let mut constraints = Vec::new();

        while !self.at(Comma) && !self.at(RightParen) {
            match self.peek() {
                // Affinity from a built in type token (INTEGER, TEXT, ...)
                Some(Integer) | Some(Text) | Some(Float) | Some(Blob) => {
                    let kind = self.next_token().unwrap().kind;
                    self.set_affinity(&mut affinity, Affinity::from(kind), &name)?;
                }
                // Affinity from any other type name (VARCHAR, DECIMAL, ...),
                // including an optional size suffix like (255) or (10, 2)
                // TODO: Fix this later
                Some(Identifier(type_name)) => {
                    let type_name = type_name.clone();
                    self.next_token();
                    self.eat_type_size()?;
                    self.set_affinity(&mut affinity, Affinity::from_type_name(&type_name), &name)?;
                }
                Some(Primary) => {
                    self.expect(Primary)?;
                    self.expect(Key)?;
                    constraints.push(Constraint::PrimaryKey);
                }
                Some(Unique) => {
                    self.expect(Unique)?;
                    constraints.push(Constraint::Unique);
                }
                Some(Not) => {
                    self.expect(Not)?;
                    self.expect(Null)?;
                    constraints.push(Constraint::NotNull);
                }
                Some(NotNull) => {
                    self.expect(NotNull)?;
                    constraints.push(Constraint::NotNull);
                }
                _ => break,
            }
        }

        let affinity = affinity.ok_or_else(|| {
            SqliteError::RuntimeError(format!("Column '{name}' is missing a data type"))
        })?;

        Ok(Column {
            name,
            affinity,
            constraints: if constraints.is_empty() {
                None
            } else {
                Some(constraints)
            },
        })
    }

    fn set_affinity(
        &mut self,
        slot: &mut Option<Affinity>,
        affinity: Affinity,
        name: &str,
    ) -> Result<(), SqliteError> {
        if slot.is_some() {
            return Err(SqliteError::RuntimeError(format!(
                "duplicate data type for column '{name}'"
            )));
        }
        *slot = Some(affinity);
        Ok(())
    }

    fn eat_type_size(&mut self) -> Result<(), SqliteError> {
        if !self.eat(LeftParen) {
            return Ok(());
        }
        while !self.at(RightParen) {
            match self.next_token() {
                Some(t) if matches!(t.kind, NumberVar(_)) => {}
                _ => {
                    return Err(SqliteError::RuntimeError(
                        "invalid token in type size".into(),
                    ));
                }
            }
            if !self.eat(Comma) && !self.at(RightParen) {
                return Err(SqliteError::RuntimeError(
                    "expected ',' or ')' in type size".into(),
                ));
            }
        }
        self.expect(RightParen)
    }
}

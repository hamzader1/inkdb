use std::rc::Rc;

use super::ast::Constraint;
use super::ast::*;
use super::parser::Parser;
use super::tokens::TokenKind::*;
use crate::errors::SqliteError;

impl Parser {
    pub fn parse_create(&mut self) -> Result<Ast, SqliteError> {
        self.expect(Create)?;
        let unique = self.eat(Unique);
        match self.peek() {
            Some(Table) => {
                if unique {
                    return Err(SqliteError::Runtime(
                        "CREATE UNIQUE TABLE is invalid: UNIQUE applies to CREATE INDEX, not CREATE TABLE".into(),
                    ));
                }
                self.parse_create_table()
            }
            Some(Index) => self.parse_create_index(unique),
            _ => Err(SqliteError::Runtime(
                "Expected TABLE or INDEX after CREATE (e.g. CREATE TABLE ... or CREATE INDEX ...)"
                    .into(),
            )),
        }
    }

    fn parse_create_table(&mut self) -> Result<Ast, SqliteError> {
        self.expect(Table)?;
        let name = self.expect_ident()?.to_ascii_lowercase();
        self.expect(LeftParen)?;
        let mut columns: Vec<Column> = Vec::new();
        while !self.at(RightParen) {
            columns.push(self.parse_create_column()?);
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect(RightParen)?;
        Ok(Ast::CreateTableAst(CreateTable {
            query: Rc::clone(&self.query),
            name,
            columns,
        }))
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
        let name = self.expect_ident()?.to_ascii_lowercase();
        let mut affinity: Option<Affinity> = None;
        let mut constraints = Vec::new();

        while !self.at(Comma) && !self.at(RightParen) {
            match self.peek() {
                // Affinity from a built in type token (INTEGER, TEXT, ...)
                Some(Integer) | Some(Text) | Some(Float) | Some(Blob) | Some(Bool) => {
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
            SqliteError::Runtime(format!("Column '{name}' is missing a data type: expected INTEGER, TEXT, FLOAT, BOOL or BLOB"))
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
            return Err(SqliteError::Runtime(format!(
                "Duplicate data type for column '{name}': each column takes exactly one type"
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
                    return Err(SqliteError::Runtime(
                        "Invalid token in type size: expected a number like VARCHAR(100)".into(),
                    ));
                }
            }
            if !self.eat(Comma) && !self.at(RightParen) {
                return Err(SqliteError::Runtime(
                    "Expected ',' or ')' in type size: e.g. DECIMAL(10, 2)".into(),
                ));
            }
        }
        self.expect(RightParen)
    }
}

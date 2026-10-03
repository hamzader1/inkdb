use super::ast::Constraint;
use super::ast::*;
use super::parser::Parser;
use super::tokens::TokenKind::*;
use crate::InkResult;
use crate::errors::InkError;

impl Parser {
    pub(crate) fn parse_create(&mut self) -> Result<Ast, InkError> {
        self.expect(Create)?;
        let unique = self.eat(Unique);
        match self.peek() {
            Some(Table) => {
                if unique {
                    return Err(InkError::runtime(
                        "CREATE UNIQUE TABLE is invalid: UNIQUE applies to CREATE INDEX, not CREATE TABLE",
                    ));
                }
                self.parse_create_table()
            }
            Some(Index) => self.parse_create_index(unique),
            _ => Err(InkError::runtime(
                "Expected TABLE or INDEX after CREATE (e.g. CREATE TABLE ... or CREATE INDEX ...)",
            )),
        }
    }
    pub(crate) fn parse_drop(&mut self) -> InkResult<Ast> {
        self.expect(Drop)?;
        match self.peek() {
            Some(Table) => self.parse_drop_table(),
            Some(Index) => self.parse_drop_index(),
            _ => Err(InkError::runtime(
                "Expected TABLE or INDEX after DROP (e.g. DROP TABLE t or DROP INDEX i)",
            )),
        }
    }

    fn parse_create_table(&mut self) -> Result<Ast, InkError> {
        self.expect(Table)?;
        let name = self.expect_ident()?.to_ascii_lowercase();
        self.expect(LeftParen)?;
        let mut columns: Vec<Column> = Vec::new();
        let mut tbl_constraints = Vec::new();
        while !self.at(RightParen) {
            columns.push(self.parse_create_column()?);
            if !self.eat(Comma) {
                break;
            }
            if self.eat(Check) {
                let tbl_cst = self.parse_expression()?;
                tbl_constraints.push(tbl_cst);
            }
        }
        self.expect(RightParen)?;
        Ok(Ast::CreateTableAst(CreateTableStmt {
            query: std::mem::take(&mut self.query),
            name,
            columns,
            tbl_constraints,
            arena: self.arena.take(),
        }))
    }

    fn parse_create_index(&mut self, unique: bool) -> Result<Ast, InkError> {
        self.expect(Index)?;
        if self.eat(If) {
            self.expect(Not)?;
            self.expect(Exists)?;
        }
        let name = self.expect_ident()?.to_ascii_lowercase();
        self.expect(On)?;
        let table = self.expect_ident()?.to_ascii_lowercase();
        self.expect(LeftParen)?;
        let mut columns = Vec::new();
        while !self.at(RightParen) {
            columns.push(self.expect_ident()?.to_ascii_lowercase());
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect(RightParen)?;
        Ok(Ast::CreateIndexAst(CreateIndexStmt {
            query: std::mem::take(&mut self.query),
            unique,
            name,
            table,
            columns,
        }))
    }

    /// not parse_columns since [`SelectStmt`] (and Insert later) reserved it
    fn parse_create_column(&mut self) -> Result<Column, InkError> {
        let name = self.expect_ident()?.to_ascii_lowercase();
        let mut affinity: Option<Affinity> = None;
        let mut constraints = Vec::new();
        let mut default = None;

        while !self.at(Comma) && !self.at(RightParen) {
            match self.peek() {
                Some(Integer) | Some(Text) | Some(Float) | Some(Blob) | Some(Bool) => {
                    let kind = self.next_token().unwrap().kind;
                    self.set_affinity(&mut affinity, Affinity::from(kind), &name)?;
                }
                Some(Identifier(type_name)) => {
                    let type_name = type_name.clone();
                    self.next_token();
                    self.eat_type_size()?;
                    self.set_affinity(&mut affinity, Affinity::from_type_name(&type_name)?, &name)?;
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
                Some(Default) => {
                    self.eat(Default);
                    if default.is_some() {
                        return Err(InkError::runtime("default value already assigned"));
                    }
                    default = Some(DefaultValue::Node(self.parse_expression()?));
                }
                _ => break,
            }
        }

        let affinity = affinity.ok_or_else(|| {
            InkError::runtime(format!("Column '{name}' is missing a data type: expected INTEGER, TEXT, FLOAT, BOOL or BLOB"))
        })?;

        Ok(Column {
            name,
            affinity,
            constraints: if constraints.is_empty() {
                None
            } else {
                Some(constraints.into())
            },
            default,
        })
    }

    fn set_affinity(
        &mut self,
        slot: &mut Option<Affinity>,
        affinity: Affinity,
        name: &str,
    ) -> Result<(), InkError> {
        if slot.is_some() {
            return Err(InkError::runtime(format!(
                "Duplicate data type for column '{name}': each column takes exactly one type"
            )));
        }
        *slot = Some(affinity);
        Ok(())
    }

    fn eat_type_size(&mut self) -> Result<(), InkError> {
        if !self.eat(LeftParen) {
            return Ok(());
        }
        while !self.at(RightParen) {
            match self.next_token() {
                Some(t) if matches!(t.kind, NumberVar(_)) => {}
                _ => {
                    return Err(InkError::runtime(
                        "Invalid token in type size: expected a number like VARCHAR(100)",
                    ));
                }
            }
            if !self.eat(Comma) && !self.at(RightParen) {
                return Err(InkError::runtime(
                    "Expected ',' or ')' in type size: e.g. DECIMAL(10, 2)",
                ));
            }
        }
        self.expect(RightParen)
    }

    pub(crate) fn parse_drop_table(&mut self) -> InkResult<Ast> {
        self.expect(Table)?;
        let tbl_name = self.expect_ident()?.to_ascii_lowercase();
        self.expect_eof()?;
        Ok(Ast::DropTblAst(DropTableStmt { tbl_name }))
    }

    pub(crate) fn parse_drop_index(&mut self) -> InkResult<Ast> {
        self.expect(Index)?;
        let index_name = self.expect_ident()?.to_ascii_lowercase();
        self.expect_eof()?;
        Ok(Ast::DropIndexAst(DropIndexStmt { index_name }))
    }
}

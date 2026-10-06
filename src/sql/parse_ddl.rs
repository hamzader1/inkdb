use super::ast::Constraint;
use super::ast::*;
use super::parser::Parser;
use super::tokens::TokenKind::*;
use crate::InkResult;
use crate::errors::InkError;

/// The statements that build and remove schema objects.
impl Parser {
    /// Read a `CREATE` statement, either a table or an index.
    ///
    /// # Errors
    /// An error when the keyword after `CREATE` is neither `TABLE` nor `INDEX`,
    /// and when a table or index is created with `UNIQUE` in front of it, which
    /// only an index may have.
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
    /// Read a `DROP` statement, either a table or an index.
    ///
    /// # Errors
    /// An error when the keyword after `DROP` is neither `TABLE` nor `INDEX`,
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

    /// Read a table definition: the column list, the column types, and the
    /// constraints written on each column.
    fn parse_create_table(&mut self) -> Result<Ast, InkError> {
        self.expect(Table)?;
        let name = self.expect_ident()?.to_ascii_lowercase();
        self.expect(LeftParen)?;
        let mut columns: Vec<Column> = Vec::new();
        let mut tbl_constraints = Vec::new();
        while !self.at(RightParen) {
            columns.push(self.parse_create_column()?);
            if self.eat(Check) {
                let tbl_cst = self.parse_expression()?;
                tbl_constraints.push(tbl_cst);
            }
            if !self.eat(Comma) {
                break;
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

    /// Read an index definition. `unique` is already known by the time this is
    /// called, because it is the keyword that came before `INDEX`.
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

    /// Read one column of a table: its name, its type, and whatever constraints
    /// follow the type.
    /// The name and type is required.
    ///
    /// # Errors
    /// An error when the name or the type is missing, and when the column is
    /// declared with an empty type size such as `INTEGER()`. Named
    /// `parse_create_column` rather than `parse_column` because the name
    /// `parse_columns` already belongs to the select list.
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

    /// Work out the type of a column from the words written for it.
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

    /// Read the optional size in brackets after a type, as in `DECIMAL(10)` or
    /// `DECIMAL(10, 2)`.
    ///
    /// A size wider than the column can hold affects nothing here, since the
    /// value stored is what decides how much room it needs, so the numbers are
    /// read and then left behind. Reading them still matters, because a type
    /// written with a size has to be accepted rather than rejected as junk.
    ///
    /// # Errors
    /// An error when the brackets are left unclosed or hold something other than
    /// a comma between the numbers.
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

    /// Read the name of the table to drop.
    pub(crate) fn parse_drop_table(&mut self) -> InkResult<Ast> {
        self.expect(Table)?;
        let tbl_name = self.expect_ident()?.to_ascii_lowercase();
        self.expect_eof()?;
        Ok(Ast::DropTblAst(DropTableStmt { tbl_name }))
    }

    /// Read the name of the index to drop.
    pub(crate) fn parse_drop_index(&mut self) -> InkResult<Ast> {
        self.expect(Index)?;
        let index_name = self.expect_ident()?.to_ascii_lowercase();
        self.expect_eof()?;
        Ok(Ast::DropIndexAst(DropIndexStmt { index_name }))
    }
}

use crate::pager::pager::Pager;
use crate::record::Value;
use crate::sql::lexer::Lexer;
use crate::sql::parser::Parser;
use crate::storage::btree::BTreeCursor;
use crate::{errors::SqliteError, sql::ast::Constraint};
use std::collections::HashMap;

use crate::sql::ast::{
    Affinity,
    Ast::{self, CreateIndexAst, CreateTableAst},
    Column, CreateIndex, CreateTable,
};

#[derive(Debug)]
pub struct Table {
    name: String,
    pub root_page: u32,
    columns: Vec<Column>,
}
impl Table {
    // pub fn is_col_exists(&self, col_name: &str) -> bool {
    //     self.columns
    //         .iter()
    //         .find(|col| col_name == col.name)
    //         .is_some()
    // }

    pub fn get_col_idx(&self, col_name: &str) -> Option<usize> {
        for (i, col) in self.columns.iter().enumerate() {
            if col.name == col_name {
                return Some(i);
            }
        }
        None
    }

    pub fn has_int_primary_key(&self) -> bool {
        for col in self.columns.iter() {
            if col.affinity == Affinity::Int
                && let Some(ref constraits) = col.constraints
            {
                return constraits
                    .iter()
                    .find(|constrait| **constrait == Constraint::PrimaryKey)
                    .is_some();
            }
        }
        false
    }
}

#[derive(Debug)]
pub struct Index {
    name: String,  // name of the index
    table: String, // name of the table
    pub root_page: u32,
    columns: Vec<String>, // single/multi col index
    unique: bool,         // is unique
}

#[derive(Debug)]
pub struct SqliteMaster {
    pub tables: HashMap<String, Table>,
    pub indexes: HashMap<String, Index>,
}

impl SqliteMaster {
    pub fn new(pager: &mut Pager) -> Result<Self, SqliteError> {
        let mut sqlite_master = Self {
            tables: HashMap::new(),
            indexes: HashMap::new(),
        };
        let mut btree_cursor = BTreeCursor::new(1);
        btree_cursor.first(pager)?;
        while let Some(record) = btree_cursor.current_record(pager)? {
            sqlite_master.parse_record(&record)?;
            btree_cursor.next(pager)?;
        }
        Ok(sqlite_master)
    }

    fn parse_record(&mut self, record: &[Value]) -> Result<(), SqliteError> {
        if record.len() != 5 {
            return Err(SqliteError::Corrupt(
                "sqlite_master record must have 5 columns".into(),
            ));
        }
        // Only 'table' and 'index' rows carry DDL we can parse. Views,
        // triggers, and internal rows (e.g. sqlite_autoindex_*) are skipped.
        let record_type = match &record[0] {
            Value::Text(t) => t.as_ref(),
            _ => return Ok(()),
        };
        if record_type != "table" && record_type != "index" {
            return Ok(());
        }
        // Auto indexes have no SQL attached.
        let sql = match &record[4] {
            Value::Text(t) => t.to_string(),
            _ => return Ok(()),
        };
        let ast = Parser::parse(Lexer::tokenize(&sql)?)?;
        self.parse_from_ast(ast, record)
    }

    fn parse_from_ast(&mut self, ast: Ast, record: &[Value]) -> Result<(), SqliteError> {
        match ast {
            CreateTableAst(ast) => {
                let table = Table {
                    name: ast.name,
                    root_page: record[3].get_int()? as _,
                    columns: ast.columns,
                };
                self.tables.insert(table.name.clone(), table);
            }
            CreateIndexAst(ast) => {
                let index = Index {
                    name: ast.name,
                    table: ast.table,
                    root_page: record[3].get_int()? as _,
                    columns: ast.columns,
                    unique: ast.unique,
                };
                self.indexes.insert(index.name.clone(), index);
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

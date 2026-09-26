use crate::SqliteResult;
use crate::backend::analyze::IndexMetadata;
use crate::errors::CorruptError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::sql::lexer::Lexer;
use crate::sql::parser::Parser;
use crate::storage::btree::{BTreeCursor, TableLeaf};
use crate::{errors::SqliteError, sql::ast::Constraint};
use std::collections::HashMap;
use std::rc::Rc;

use crate::sql::ast::{
    Affinity,
    Ast::{self, CreateIndexAst, CreateTableAst},
    Column,
};

#[derive(Debug)]
pub struct Table {
    pub name: String,
    pub root_page: u32,
    pub columns: Vec<Column>,
}

use std::sync::LazyLock;

pub static SQLITE_MASTER: LazyLock<Table> = LazyLock::new(|| Table {
    name: "sqlite_master".to_string(),
    root_page: 1,
    columns: vec![
        Column {
            name: "type".to_string(),
            affinity: Affinity::Text,
            constraints: None,
        },
        Column {
            name: "name".to_string(),
            affinity: Affinity::Text,
            constraints: None,
        },
        Column {
            name: "tbl_name".to_string(),
            affinity: Affinity::Text,
            constraints: None,
        },
        Column {
            name: "rootpage".to_string(),
            affinity: Affinity::Int,
            constraints: None,
        },
        Column {
            name: "sql".to_string(),
            affinity: Affinity::Text,
            constraints: None,
        },
    ],
});
impl Table {
    pub fn get_col_idx(&self, col_name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == col_name)
    }
    pub fn get_col_name(&self, idx: usize) -> Option<&Column> {
        self.columns.get(idx)
    }
    pub fn get_cols_len(&self) -> usize {
        self.columns.len()
    }

    pub fn has_int_primary_key(&self) -> bool {
        self.columns.iter().any(|col| {
            col.constraints
                .iter()
                .any(|cts| cts.contains(&Constraint::PrimaryKey))
        })
    }
}

#[derive(Debug, Clone)]
pub struct Index {
    pub name: String,  // name of the index
    pub table: String, // name of the table
    pub root_page: u32,
    pub columns: Vec<String>, // single/multi col index
    pub unique: bool,         // is unique
}

impl Index {
    pub fn is_on(&self, col_name: &str, table_name: &str) -> bool {
        // LIMITED: for single col index
        // TODO: HANDLE MULTIPLE INDEXES
        let (indexed_col, indexed_table) = (&self.columns[0], &self.table);
        indexed_table.eq_ignore_ascii_case(table_name) && indexed_col.eq_ignore_ascii_case(col_name)
    }
}

#[derive(Debug)]
pub struct SqliteMaster {
    pub tables: HashMap<String, Table>,
    pub indexes: HashMap<String, Index>,
}

impl SqliteMaster {
    pub fn new<V: crate::vfs::Vfs>(pager: &mut Pager<V>) -> Result<Self, SqliteError> {
        let mut sqlite_master = Self {
            tables: HashMap::new(),
            indexes: HashMap::new(),
        };
        let mut btree_cursor = BTreeCursor::new(1);
        btree_cursor.first(pager)?;
        while let Some(record) = btree_cursor.current_record::<TableLeaf>(pager)? {
            sqlite_master.parse_record(&record)?;
            btree_cursor.next(pager)?;
        }
        Ok(sqlite_master)
    }

    pub fn table(&self, table_name: &str) -> Option<&Table> {
        self.tables
            .values()
            .find(|table| table.name.eq_ignore_ascii_case(table_name))
    }

    pub(crate) fn indexes_on(&self, table_name: &str) -> SqliteResult<Vec<IndexMetadata>> {
        let table = self
            .table(table_name)
            .ok_or_else(|| SqliteError::TableNotFound(table_name.to_string()))?;
        let mut indexes_of_t = Vec::new();
        for index in self.indexes.values() {
            if !index.table.eq_ignore_ascii_case(&table.name) {
                continue;
            }
            let column_idx = table.get_col_idx(&index.columns[0]).ok_or_else(|| {
                SqliteError::runtime(format!(
                    "Column {} does not exist on table {}",
                    index.columns[0], table.name
                ))
            })?;
            indexes_of_t.push(IndexMetadata::new(
                index.root_page,
                column_idx,
                index.unique,
            ));
        }
        Ok(indexes_of_t)
    }

    fn parse_record(&mut self, record: &[Value]) -> Result<(), SqliteError> {
        if record.len() != 5 {
            return Err(CorruptError::CatalogRecord {
                columns: record.len(),
            }
            .into());
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
            Value::Text(t) => t.as_ref(),
            _ => return Ok(()),
        };
        let query: Rc<str> = Rc::from(sql);
        let ast = Parser::parse(query, Lexer::tokenize(sql)?)?;
        self.parse_from_ast(ast, record)
    }

    fn parse_from_ast(&mut self, ast: Ast, record: &[Value]) -> Result<(), SqliteError> {
        match ast {
            CreateTableAst(ast) => {
                let table = Table {
                    name: ast.name,
                    root_page: record[3].cast_int()? as _,
                    columns: ast.columns,
                };
                self.tables.insert(table.name.clone(), table);
            }
            CreateIndexAst(ast) => {
                let index = Index {
                    name: ast.name,
                    table: ast.table,
                    root_page: record[3].cast_int()? as _,
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

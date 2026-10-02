use crate::InkResult;
use crate::backend::analyzer::{
    self, Analyze, IndexMetadata, ResolvedCreateTableQuery, ResolvedQuery,
};
use crate::backend::executor::eval::Eval;
use crate::errors::CorruptError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::sql::ast::{CreateTableStmt, DefaultValue};
use crate::sql::lexer::Lexer;
use crate::sql::parser::{ExprArena, Parser};
use crate::storage::btree::{BTreeCursor, TableLeaf};
use crate::vfs::Vfs;
use crate::{errors::InkError, sql::ast::Constraint};
use std::collections::HashMap;
use std::rc::Rc;

use crate::sql::ast::{
    Affinity,
    Ast::{self, CreateIndexAst, CreateTableAst},
    Column,
};

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub root_page: u32,
    pub columns: Vec<Column>,
    pub tbl_constraits: Vec<usize>,
    pub tbl_arena: ExprArena,
}

use std::sync::LazyLock;

pub static MASTER: LazyLock<Table> = LazyLock::new(|| Table {
    name: "master".to_string(),
    root_page: 1,
    columns: vec![
        Column {
            name: "type".to_string(),
            affinity: Affinity::Text,
            constraints: None,
            default: None,
        },
        Column {
            name: "name".to_string(),
            affinity: Affinity::Text,
            constraints: None,
            default: None,
        },
        Column {
            name: "tbl_name".to_string(),
            affinity: Affinity::Text,
            constraints: None,
            default: None,
        },
        Column {
            name: "rootpage".to_string(),
            affinity: Affinity::Int,
            constraints: None,
            default: None,
        },
        Column {
            name: "sql".to_string(),
            affinity: Affinity::Text,
            constraints: None,
            default: None,
        },
    ],
    tbl_arena: ExprArena::new(),
    tbl_constraits: Vec::new(),
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

    pub fn has_integer_primary_key(&self) -> Option<usize> {
        for col in self.columns.iter() {
            for ct in col.constraints.iter() {
                if let Some(idx) = ct.iter().position(|c| c == &Constraint::PrimaryKey) {
                    return Some(idx);
                }
            }
        }
        None
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
pub struct Master {
    pub tables: HashMap<String, Table>,
    pub indexes: HashMap<String, Index>,
    pub is_dirty: bool,
}

impl Master {
    pub fn new<V: crate::vfs::Vfs>(pager: &mut Pager<V>) -> Result<Self, InkError> {
        let mut master = Self {
            tables: HashMap::new(),
            indexes: HashMap::new(),
            is_dirty: false,
        };
        master.parse(pager)?;
        Ok(master)
    }

    pub fn parse<V: Vfs>(&mut self, pager: &mut Pager<V>) -> InkResult<()> {
        /*
         * Clearing indexes & tables in case master table was dirty and
         * needs to fetch the new update from disk
         */
        self.indexes.clear();
        self.tables.clear();
        let mut btree_cursor = BTreeCursor::new(1);
        btree_cursor.first(pager)?;
        while let Some(record) = btree_cursor.current_record::<TableLeaf>(pager)? {
            self.parse_record(&record)?;
            btree_cursor.next(pager)?;
        }
        self.is_dirty = false; /* We are having the latest update */
        Ok(())
    }
    pub fn table(&self, table_name: &str) -> Option<&Table> {
        self.tables
            .values()
            .find(|table| table.name.eq_ignore_ascii_case(table_name))
    }

    pub(crate) fn indexes_on(&self, table_name: &str) -> InkResult<Vec<IndexMetadata>> {
        if table_name.eq_ignore_ascii_case("master") {
            /*Change this to Option*/
            return Ok(vec![]);
        }
        let table = self
            .table(table_name)
            .ok_or_else(|| InkError::TableNotFound(table_name.to_string()))?;
        let mut indexes_of_t = Vec::new();
        for index in self.indexes.values() {
            if !index.table.eq_ignore_ascii_case(&table.name) {
                continue;
            }
            let column_idx = table.get_col_idx(&index.columns[0]).ok_or_else(|| {
                InkError::runtime(format!(
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

    fn parse_record(&mut self, record: &[Value]) -> Result<(), InkError> {
        if record.len() != 5 {
            return Err(CorruptError::CatalogRecord {
                columns: record.len(),
            }
            .into());
        }
        // Only 'table' and 'index' rows carry DDL we can parse. Views,
        // triggers, and internal rows (e.g. autoindex_*) are skipped.
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

    fn parse_from_ast(&mut self, ast: Ast, record: &[Value]) -> Result<(), InkError> {
        match ast {
            CreateTableAst(ast) => {
                let analyzer = Analyze::new(self).analyze_create_table_stmt(ast)?;
                let ResolvedQuery::CreateTableQuery(mut q) = analyzer else {
                    unreachable!()
                };
                let mut table = Table {
                    name: q.meta.name,
                    root_page: record[3].cast_int()? as _,
                    columns: q.meta.columns,
                    tbl_constraits: q.meta.tbl_constraints,
                    tbl_arena: q.meta.arena,
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

pub trait TableSchema: std::fmt::Debug + Clone {
    fn column_index(&self, col_name: &str) -> Option<usize>;
    fn column_name(&self, col_idx: usize) -> Option<&Column>;
    fn columns_len(&self) -> usize;
}
impl TableSchema for Table {
    fn column_index(&self, col_name: &str) -> Option<usize> {
        self.get_col_idx(col_name)
    }
    fn column_name(&self, col_idx: usize) -> Option<&Column> {
        self.get_col_name(col_idx)
    }
    fn columns_len(&self) -> usize {
        self.get_cols_len()
    }
}

impl TableSchema for CreateTableStmt {
    fn column_index(&self, col_name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == col_name)
    }
    fn column_name(&self, col_idx: usize) -> Option<&Column> {
        self.columns.get(col_idx)
    }
    fn columns_len(&self) -> usize {
        self.columns.len()
    }
}

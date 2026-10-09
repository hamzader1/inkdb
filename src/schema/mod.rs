use crate::InkResult;
use crate::backend::analyzer::{Analyze, IndexMetadata, ResolvedQuery};
use crate::errors::CorruptError;
use crate::errors::InkError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::sql::ast::CreateTableStmt;
use crate::sql::lexer::Lexer;
use crate::sql::parser::{ExprArena, Parser};
use crate::storage::btree::{BTreeCursor, TableLeaf};
use crate::vfs::Vfs;
use std::collections::HashMap;
use std::rc::Rc;

use crate::sql::ast::{
    Affinity,
    Ast::{self, CreateIndexAst, CreateTableAst},
    Column,
};

/// A table, as the catalog describes it.
///
/// The columns and their constraints come from the master table, so a table is a
/// copy of what is stored rather than something held in memory only. The arena
/// belongs to this table and holds the expressions of its own constraints.
#[derive(Debug, Clone)]
pub struct Table {
    name: String,
    root_page: u32,
    columns: Box<[Column]>,
    tbl_constraits: Box<[usize]>,
    tbl_arena: ExprArena,
}

impl Table {
    /// The table name.
    pub fn name(&self) -> &String {
        &self.name
    }

    /// The page the table's btree/b+tree starts at.
    pub fn root_page(&self) -> u32 {
        self.root_page
    }

    /// The columns, in the order the table declares them.
    pub(crate) fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// The table level constraints, held as indices into the arena below.
    pub(crate) fn constraints(&self) -> &[usize] {
        &self.tbl_constraits
    }

    /// The expressions of the table level constraints.
    pub(crate) fn arena(&self) -> &ExprArena {
        &self.tbl_arena
    }
}

use std::sync::LazyLock;

/// The master table itself, described in code.
///
/// Every database has one and it cannot be read from the catalog, because it is
/// the catalog. Its five columns are the ones a SQLite file gives it, and page
/// one is always its root.
/// ```text
/// +----------+----------+----------+----------+----------+
/// |   type   |   name   | tbl_name |root_page |   sql    |
/// +----------+----------+----------+----------+----------+
/// ```
pub(crate) static MASTER: LazyLock<Table> = LazyLock::new(|| Table {
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
    ]
    .into(),
    tbl_arena: ExprArena::new(),
    tbl_constraits: [].into(),
});
impl Table {
    /// The position of the column with this name, or nothing when there is no
    /// such column.
    pub fn get_col_idx(&self, col_name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == col_name)
    }

    /// The column at this position.
    pub(crate) fn get_col_name(&self, idx: usize) -> Option<&Column> {
        self.columns.get(idx)
    }

    /// How many columns the table has.
    pub fn get_cols_len(&self) -> usize {
        self.columns.len()
    }

    /// The position of the column that stands in for the row id, if the table
    /// has one.
    ///
    /// A single `INTEGER PRIMARY KEY` column aliases the row id, so its value and
    /// the row id are the same thing and the row id is not stored twice.
    pub fn rowid_column(&self) -> Option<usize> {
        self.columns
            .iter()
            .position(|column| column.has_primary_key())
    }
}

/// An index, as the catalog describes it.
#[derive(Debug, Clone)]
pub struct Index {
    name: String,
    table: String,
    root_page: u32,
    columns: Box<[String]>,
    unique: bool,
}

impl Index {
    /// The page the index b-tree starts at.
    pub fn root_page(&self) -> u32 {
        self.root_page
    }

    /// The index name.
    pub(crate) fn name(&self) -> &String {
        &self.name
    }

    /// Whether this index covers that column of that table.
    ///
    /// Only the first column is looked at, because indexes are single column
    /// today. A multi column index will need this to compare the whole list.
    pub fn is_on(&self, col_name: &str, table_name: &str) -> bool {
        let (indexed_col, indexed_table) = (&self.columns[0], &self.table);
        indexed_table.eq_ignore_ascii_case(table_name) && indexed_col.eq_ignore_ascii_case(col_name)
    }
}

/// The schema of one database: every table and every index.
///
/// This is the catalog read into memory. It is kept in step with the file rather
/// than being the truth itself, so a statement that changes the schema marks it
/// stale (is_dirty=true) and the next statement has it read again.
#[derive(Debug)]
pub struct Master {
    tables: HashMap<String, Table>,
    indexes: HashMap<String, Index>,
    is_dirty: bool,
}

impl Master {
    /// Every table the catalog holds, keyed by name.
    pub fn tables(&self) -> &HashMap<String, Table> {
        &self.tables
    }

    /// Every index the catalog holds, keyed by name.
    pub fn indexes(&self) -> &HashMap<String, Index> {
        &self.indexes
    }

    /// Whether a statement has changed the schema since this was last read.
    pub(crate) fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    /// Note that the schema on disk changed, so it has to be read again before
    /// anything else is planned.
    pub(crate) fn mark_dirty(&mut self) {
        self.is_dirty = true;
    }
}

impl Master {
    /// Read the catalog from the master table.
    ///
    /// # Errors
    /// Whatever walking the master table reports, such as a page that cannot be
    /// read or a catalog row whose DDL does not parse.
    pub fn new<V: crate::vfs::Vfs>(pager: &mut Pager<V>) -> Result<Self, InkError> {
        let mut master = Self {
            tables: HashMap::new(),
            indexes: HashMap::new(),
            is_dirty: false,
        };
        master.parse(pager)?;
        Ok(master)
    }

    /// Walk the master table and build the schema from it again.
    ///
    /// # Errors
    /// Whatever the walk over the master table reports, including any row it
    /// cannot make sense of.
    pub fn parse<V: Vfs>(&mut self, pager: &mut Pager<V>) -> InkResult<()> {
        self.indexes.clear();
        self.tables.clear();
        let mut btree_cursor = BTreeCursor::new(1);
        btree_cursor.first(pager)?;
        while let Some(record) = btree_cursor.current_record::<TableLeaf>(pager)? {
            self.parse_record(&record)?;
            btree_cursor.next(pager)?;
        }
        self.is_dirty = false;
        Ok(())
    }

    /// The table with this name, ignoring case, or nothing when there is none.
    ///
    /// Names are matched without regard to case, so `users`, `Users` and `USERS`
    /// all find the same table, which is how SQL names behave.
    pub fn table(&self, table_name: &str) -> Option<&Table> {
        self.tables
            .values()
            .find(|table| table.name.eq_ignore_ascii_case(table_name))
    }

    /// The index with this name, ignoring case, or nothing when there is none.
    pub fn index(&self, index_name: &str) -> Option<&Index> {
        self.indexes
            .values()
            .find(|index| index.name.eq_ignore_ascii_case(index_name))
    }

    /// Every index that belongs to this table, ready for the planner to use.
    ///
    /// The master table has none, so that is answered without looking anything
    /// up.
    ///
    /// # Errors
    /// [`InkError::TableNotFound`] when there is no such table.
    pub(crate) fn indexes_on(&self, table_name: &str) -> InkResult<Vec<IndexMetadata>> {
        if table_name.eq_ignore_ascii_case("master") {
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

    /// Turn one row of the master table into a table or an index.
    ///
    /// Rows that describe something else, such as views and triggers, and rows
    /// with no SQL attached, such as automatic indexes, are skipped rather than
    /// treated as errors: they belong to other databases and cause no harm here.
    ///
    /// # Errors
    /// [`CorruptError::CatalogRecord`] when the row does not have the five
    /// columns the master table is supposed to have.
    fn parse_record(&mut self, record: &[Value]) -> Result<(), InkError> {
        if record.len() != 5 {
            return Err(CorruptError::CatalogRecord {
                columns: record.len(),
            }
            .into());
        }
        let record_type = match &record[0] {
            Value::Text(t) => t.as_ref(),
            _ => return Ok(()),
        };
        if record_type != "table" && record_type != "index" {
            return Ok(());
        }
        let sql = match &record[4] {
            Value::Text(t) => t.as_ref(),
            _ => return Ok(()),
        };
        let query: Rc<str> = Rc::from(sql);
        let ast = Parser::parse(query, Lexer::tokenize(sql)?)?;
        self.parse_from_ast(ast, record)
    }

    /// Build the table or index a master row describes.
    ///
    /// The DDL is run back through the parser and the analyzer, which is why a
    /// table arrives here with its columns already worked out and its constraints
    /// already bound. The page number comes from the row itself, since that is
    /// the one thing the DDL does not say.
    ///
    fn parse_from_ast(&mut self, ast: Ast, record: &[Value]) -> Result<(), InkError> {
        match ast {
            CreateTableAst(ast) => {
                let analyzer = Analyze::new(self).analyze_create_table_stmt(ast)?;
                let ResolvedQuery::CreateTableQuery(q) = analyzer else {
                    unreachable!()
                };
                let table = Table {
                    name: q.meta.name,
                    root_page: record[3].cast_int()? as _,
                    columns: q.meta.columns.into(),
                    tbl_constraits: q.meta.tbl_constraints.into(),
                    tbl_arena: q.meta.arena,
                };
                self.tables.insert(table.name.clone(), table);
            }
            CreateIndexAst(ast) => {
                let index = Index {
                    name: ast.name,
                    table: ast.table,
                    root_page: record[3].cast_int()? as _,
                    columns: ast.columns.into(),
                    unique: ast.unique,
                };
                self.indexes.insert(index.name.clone(), index);
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

/// Looking a column up by name or by position.
///
/// A table read from the catalog and a table definition still being parsed both
/// answer these three questions, and the code that binds names to columns works
/// against either one.
pub(crate) trait TableSchema: std::fmt::Debug + Clone {
    /// The position of the column with this name.
    fn column_index(&self, col_name: &str) -> Option<usize>;
    /// The column at this position.
    #[allow(dead_code)]
    fn column_name(&self, col_idx: usize) -> Option<&Column>;
    /// How many columns there are.
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
    /// The same three lookups again, for a table that is still being defined, so
    /// that a statement can refer to a column of a table that does not exist yet.
    /// This is mainly used by the analyzer step ([Master::parse_from_ast]), since
    /// the analyzer needs to perform internal checks, such as evaluating the given
    /// DEFAULT value (if any) from the query.
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

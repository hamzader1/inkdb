use crate::backend::analyze::{ResolvedCreateIndexQuery, ResolvedCreateTableQuery};
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::{SqlType, Value};
use crate::storage::btree::BTree;
use crate::storage::page::{BTreePageMut, BTreePageType};
use crate::vfs::file::SqliteFile;

use super::Row;
use super::insert::Insert;
use super::prepare::PrepareRow;
use crate::record::tuple::Tuple;
use crate::storage::cell::Encode;

#[derive(Debug)]
pub struct CreateTable {
    meta: ResolvedCreateTableQuery,
}
impl CreateTable {
    pub fn table_name(&self) -> &str {
        &self.meta.meta.name
    }
}
#[derive(Debug)]
pub struct CreateIndex<F: SqliteFile> {
    child: Box<Plan<F>>,
    index_root_page: u32,
    col_idx: usize, // todo: remake usize -> Vec::<usize>;
}
impl<F: SqliteFile> CreateIndex<F> {
    pub fn new(
        child: Box<Plan<F>>,
        meta: ResolvedCreateIndexQuery,
        pager: &mut Pager<impl SqliteFile>,
    ) -> Result<Self, SqliteError> {
        // todo: start txn
        let new_page = pager.allocate_new_page()?;
        let mut guard = pager.get_mut(new_page)?;
        let bytes = guard.bytes_as_mut_unchecked();
        BTreePageMut::new_from_raw_bytes(
            new_page,
            BTreePageType::LeafIndex,
            bytes,
            pager.metadata.page_size,
            pager.metadata.usable_size,
        );
        let row = [
            Value::text("index"),
            Value::text(&meta.index_name),
            Value::text(&meta.relation_name),
            Value::Integer(new_page as _),
            Value::text(&meta.query),
        ];

        let mut prepare = PrepareRow::new(
            None,
            1,
            vec![row.iter().map(|v| v.into_owned()).collect()],
            None,
        );
        while prepare.next(pager)?.is_some() {}
        Ok(Self {
            child,
            index_root_page: new_page,
            col_idx: meta.column_index,
        })
    }
    pub fn index_root_page(&self) -> u32 {
        self.index_root_page
    }
    pub fn col_idx(&self) -> usize {
        self.col_idx
    }
    // todo: remove allocte per insert.
    // use batch instead
    pub fn next(&mut self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        while let Some(row) = self.child.next(pager, None)? {
            let record = [row[self.col_idx].clone(), row.key.into_sqlite_value()];
            let key = Value::Tuple(record.to_vec());
            let mut bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&record));
            Insert::new(self.index_root_page, key, &mut bytes).next(pager)?;
        }
        Ok(None)
    }
}

impl CreateTable {
    pub fn new(meta: ResolvedCreateTableQuery) -> Self {
        Self { meta }
    }

    pub fn next<F: SqliteFile>(&self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        let is_new_txn = pager.start_transaction();
        let name = &self.meta.meta.name;
        // Allocating a new page
        let new_page = BTree::new(1, pager).allocate_page()?;
        let mut guard = pager.get_mut(new_page)?;
        let bytes = guard.bytes_as_mut_unchecked();
        BTreePageMut::new_from_raw_bytes(
            new_page,
            BTreePageType::LeafTable,
            bytes,
            pager.metadata.page_size,
            pager.metadata.usable_size,
        );
        let row = [
            Value::text("table"),                       // type
            Value::text(name),                          // name
            Value::text(name),                          // table_name
            Value::Integer(new_page as _),              // root page
            Value::text(self.meta.meta.query.as_ref()), // original query
        ];

        let mut prepare = PrepareRow::new(
            None,
            1,
            vec![row.iter().map(|v| v.into_owned()).collect()],
            None,
        );
        while prepare.next(pager)?.is_some() {}
        if is_new_txn {
            pager.commit()?;
        }
        Ok(None)
    }
}

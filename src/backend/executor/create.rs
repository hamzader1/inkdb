use crate::backend::analyze::{ResolvedCreateIndexQuery, ResolvedCreateTableQuery};
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::storage::btree::{BTree, page_as_mut_with_pager};
use crate::storage::page::{BTreePageMut, BTreePageType};
use crate::vfs::file::SqliteFile;

use super::Row;
use super::insert::Insert;

#[derive(Debug)]
pub struct CreateTable {
    meta: ResolvedCreateTableQuery,
}
#[derive(Debug)]
pub struct CreateIndex {
    index_root_page: u32,
    col_idx: usize, // todo: remake usize -> Vec::<usize>;
}
impl CreateIndex {
    pub fn new(
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

        let insert = Insert::new(1, vec![row.to_vec()], None);
        insert.next(pager)?;
        Ok(Self {
            index_root_page: new_page,
            col_idx: meta.column_index,
        })
    }
    pub fn next(&self, pager: &mut Pager<impl SqliteFile>) -> Result<Option<Row>, SqliteError> {
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

        let insert = Insert::new(1, vec![row.to_vec()], None);
        insert.next(pager)?;
        if is_new_txn {
            pager.commit()?;
        }
        Ok(None)
    }
}

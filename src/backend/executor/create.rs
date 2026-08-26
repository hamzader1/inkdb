use crate::backend::analyze::ResolvedCreateTableQuery;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::{SqlType, Value};
use crate::storage::btree::{BTree, BTreeCursor};
use crate::storage::cell::Encode;
use crate::storage::page::{BTreePageMut, BTreePageType};
use crate::varint::encode_varint;
use crate::vfs::file::SqliteFile;

use super::Row;
use super::insert::Insert;

#[derive(Debug)]
pub struct CreateTable {
    meta: ResolvedCreateTableQuery,
}
impl CreateTable {
    pub fn new(meta: ResolvedCreateTableQuery) -> Self {
        Self { meta }
    }

    pub fn next<F: SqliteFile>(&self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        let name = &self.meta.meta.name;
        // Allocating a new page
        let mut new_page = BTree::new(1, pager).allocate_page()?;
        let mut guard = pager.get_mut(new_page)?;
        let bytes = guard.bytes_as_mut_unchecked();
        let page = BTreePageMut::new_from_scratch(
            new_page,
            BTreePageType::LeafTable,
            bytes,
            pager.metadata.page_size,
            pager.metadata.usable_size,
        );
        let row = [
            Value::text("table"),
            Value::text(name),
            Value::text(name),
            Value::Integer(new_page as _),
            Value::text(self.meta.meta.query.as_ref()),
        ];

        let insert = Insert::new(1, row.to_vec(), None);
        insert.next(pager)?;
        Ok(None)
    }
}

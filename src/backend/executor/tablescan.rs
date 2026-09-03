use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::sql::parser::ExprArena;
use crate::storage::btree::{BTree, BTreeCursor, page_as_ref_with_pager};
use crate::storage::cell::BTreeCell;
use crate::storage::page::BTreePageRef;
use crate::vfs::file::SqliteFile;

use super::Row;

#[derive(Debug)]
pub struct TableScan<F: SqliteFile> {
    cursor: BTreeCursor<F>,
    is_done: bool,
}
impl<F: SqliteFile> TableScan<F> {
    pub fn new(root_page: u32, pager: &mut Pager<F>) -> Result<Self, SqliteError> {
        let mut cursor = BTreeCursor::new(root_page);
        cursor.first(pager)?;
        let (page_no, _) = cursor.last_visited_entry_unchecked();
        let guard = pager.get(page_no)?;
        let page = BTreePageRef::new(
            page_no,
            guard.bytes_as_ref(),
            pager.metadata.page_size,
            pager.metadata.usable_size,
        )?;
        let empty = page.no_of_cells() == 0;

        Ok(Self {
            cursor,
            is_done: empty,
        })
    }
}
impl<F: SqliteFile> TableScan<F> {
    pub fn next(
        &mut self,
        pager: &mut Pager<F>,
        // arena: &ExprArena,
    ) -> Result<Option<Row>, SqliteError> {
        if self.is_done {
            return Ok(None);
        }
        if let Some((page_no, cell_idx)) = self.cursor.last_visited_entry() {
            let row_id = self.cursor.with_current(pager, |_, c| Ok(c.row_id()))?;
            let record = self.cursor.current_record(pager)?.unwrap();
            let v = record.iter().map(|v| v.into_owned()).collect();
            let row = Row::new(row_id, v);
            self.cursor.next(pager)?;
            return Ok(Some(row));
        }
        self.is_done = true;
        Ok(None)
    }
}

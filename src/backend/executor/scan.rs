use crate::errors::SqliteError;
use crate::record::Value;
use crate::sql::parser::ExprArena;
use crate::storage::btree::BTreeCursor;
use crate::storage::page::BTreePageRef;

use super::Row;
use crate::pager::pager::Pager;

#[derive(Debug)]
pub struct TableScan {
    root_page: u32,
    cursor: BTreeCursor,
    is_done: bool,
}
impl TableScan {
    pub fn new(root_page: u32, pager: &mut Pager) -> Result<Self, SqliteError> {
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
            root_page,
            cursor,
            is_done: empty,
        })
    }
}
impl TableScan {
    pub fn next(
        &mut self,
        pager: &mut Pager,
        // arena: &ExprArena,
    ) -> Result<Option<Row>, SqliteError> {
        if self.is_done {
            return Ok(None);
        }
        if let Some(row) = self.cursor.current_record(pager)? {
            let v: Row = row.iter().map(|v| v.into_owned()).collect();
            self.cursor.next(pager)?;
            return Ok(Some(v));
        }
        self.is_done = true;
        Ok(None)
    }
}

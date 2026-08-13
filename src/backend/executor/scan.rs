use crate::errors::SqliteError;
use crate::record::Value;
use crate::storage::btree::BTreeCursor;

use super::{Executor, Row};
use crate::pager::pager::Pager;

pub struct TableScan {
    root_page: u32,
    cursor: BTreeCursor,
}
impl TableScan {
    pub fn new(root_page: u32, pager: &mut Pager) -> Self {
        let mut cursor = BTreeCursor::new(root_page);
        cursor.first(pager);
        dbg!(&cursor);
        Self { root_page, cursor }
    }
}
impl Executor for TableScan {
    fn next(&mut self, pager: &mut Pager) -> Result<Option<Row>, SqliteError> {
        if let Some(row) = self.cursor.current_record(pager)? {
            let v: Row = row.iter().map(|v| v.into_owned()).collect();
            self.cursor.next(pager)?;
            return Ok(Some(v));
        }
        Ok(None)
    }
}

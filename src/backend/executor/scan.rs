use crate::errors::SqliteError;
use crate::record::Value;
use crate::storage::btree::BTreeCursor;
use crate::vfs::file::SqliteFile;

use super::{Executor, Row};
use crate::pager::pager::Pager;

pub struct TableScan {
    root_page: u32,
    cursor: BTreeCursor,
}
impl Executor for TableScan {
    fn next(&mut self, pager: &mut Pager<impl SqliteFile>) -> Result<Option<Row>, SqliteError> {
        if let Some(row) = self.cursor.current_record(pager)? {
            let v: Row = row.iter().map(|v| v.into_owned()).collect();
            return Ok(Some(v));
        }
        Ok(None)
    }
}

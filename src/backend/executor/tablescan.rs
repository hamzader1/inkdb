use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::storage::btree::{BTreeCursor, RestorePosition};
use crate::storage::page::BTreePageRef;
use crate::vfs::file::SqliteFile;

use super::Row;

#[derive(Debug)]
pub struct TableScan<F: SqliteFile> {
    cursor: BTreeCursor<F>,
    is_done: bool,
    scan_plan: Box<dyn Scan<F>>,
}
impl<F: SqliteFile> TableScan<F> {
    pub fn new(
        root_page: u32,
        pager: &mut Pager<F>,
        scan_plan: Box<dyn Scan<F>>,
    ) -> Result<Self, SqliteError> {
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
            scan_plan,
        })
    }
}
impl<F: SqliteFile> TableScan<F> {
    pub fn next(&mut self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        if self.is_done {
            return Ok(None);
        }
        if let Some(r) = self.scan_plan.next(pager, &mut self.cursor)? {
            return Ok(Some(r));
        }
        self.is_done = true;
        Ok(None)
    }
}

pub trait Scan<F: SqliteFile>: std::fmt::Debug {
    fn next(
        &mut self,
        pager: &mut Pager<F>,
        cursor: &mut BTreeCursor<F>,
    ) -> Result<Option<Row>, SqliteError>;
}

#[derive(Debug)]
pub struct TableUnsafeScan;
impl<F: SqliteFile> Scan<F> for TableUnsafeScan {
    fn next(
        &mut self,
        pager: &mut Pager<F>,
        cursor: &mut BTreeCursor<F>,
    ) -> Result<Option<Row>, SqliteError> {
        // Unsafe means the row may vanish under us through Delete. When the
        // saved row is still there we step over it, when it is gone the
        // restore already parks on its successor. Either way the cursor is
        // valid here or the scan is over.
        match cursor.restore_position(pager)? {
            RestorePosition::Exact => cursor.next(pager)?,
            RestorePosition::Next | RestorePosition::Empty => {}
        }
        let Some(cell) = cursor.current(pager)? else {
            return Ok(None);
        };
        let row_id = cell.row_id();
        let Some(record) = cursor.current_record(pager)? else {
            return Ok(None);
        };
        let v = record.iter().map(|v| v.into_owned()).collect();
        let row = Row::new(row_id, v);
        cursor.save_position(pager)?;
        return Ok(Some(row));
    }
}

#[derive(Debug)]
pub struct TableSafeScan;
impl<F: SqliteFile> Scan<F> for TableSafeScan {
    fn next(
        &mut self,
        pager: &mut Pager<F>,
        cursor: &mut BTreeCursor<F>,
    ) -> Result<Option<Row>, SqliteError> {
        if cursor.last_visited_entry().is_some() {
            let row_id = cursor.with_current(pager, |_, c| Ok(c.row_id()))?;
            let record = cursor.current_record(pager)?.unwrap();
            let v = record.iter().map(|v| v.into_owned()).collect();
            let row = Row::new(row_id, v);
            cursor.next(pager)?;
            return Ok(Some(row));
        }
        Ok(None)
    }
}

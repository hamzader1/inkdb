use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::storage::btree::{BTreeCursor, RestorePosition};
use crate::storage::page::PageRef as BTreePageRef;
use crate::vfs::Vfs;

use super::Row;
use super::scan_guard::ScanGuard;

#[derive(Debug)]
pub struct TableScan<V: Vfs> {
    pub cursor: BTreeCursor<V>,
    is_done: bool,
    pub guard: Box<dyn ScanGuard<V>>,
}
impl<V: Vfs> TableScan<V> {
    pub fn new(
        root_page: u32,
        pager: &mut Pager<V>,
        scan_plan: Box<dyn ScanGuard<V>>,
    ) -> Result<Self, SqliteError> {
        let mut cursor = BTreeCursor::new(root_page);
        cursor.first(pager)?;
        let (page_no, _) = cursor.last_visited_entry_unchecked();
        let guard = pager.get(page_no)?;
        let page = BTreePageRef::new(
            page_no,
            pager.page_size(),
            pager.usable_size(),
            guard.bytes(),
        )?;
        let empty = page.no_of_cells()? == 0;

        Ok(Self {
            cursor,
            is_done: empty,
            guard: scan_plan,
        })
    }
}
impl<V: Vfs> TableScan<V> {
    pub fn next(&mut self, pager: &mut Pager<V>) -> Result<Option<Row>, SqliteError> {
        if self.is_done {
            return Ok(None);
        }
        self.guard.restore(pager, &mut self.cursor)?;
        let Some(cell) = self.cursor.current(pager)? else {
            self.is_done = true;
            return Ok(None);
        };
        let row_id = cell.row_id();
        let Some(record) = self.cursor.current_record(pager)? else {
            self.is_done = true;
            return Ok(None);
        };
        let v = record.iter().map(|v| v.into_owned()).collect();
        let row = Row::new(row_id, v);
        self.guard.save_or_advance(pager, &mut self.cursor)?;
        Ok(Some(row))
    }
}

// pub trait RelationScan<V: Vfs>: std::fmt::Debug {
//     fn next(
//         &mut self,
//         pager: &mut Pager<V>,
//         cursor: &mut BTreeCursor<V>,
//     ) -> Result<Option<Row>, SqliteError>;
// }

// #[derive(Debug)]
// pub struct UnsafeTableScan;
// impl<V: Vfs> RelationScan<V> for UnsafeTableScan {
//     fn next(
//         &mut self,
//         pager: &mut Pager<V>,
//         cursor: &mut BTreeCursor<V>,
//     ) -> Result<Option<Row>, SqliteError> {
//         // Unsafe means the row may vanish under us through Delete. When the
//         // saved row is still there we step over it, when it is gone the
//         // restore already parks on its successor. Either way the cursor is
//         // valid here or the scan is over.
//         match cursor.restore_position(pager)? {
//             RestorePosition::Exact => cursor.next(pager)?,
//             RestorePosition::Next | RestorePosition::Empty => {}
//         }
//         let Some(cell) = cursor.current(pager)? else {
//             return Ok(None);
//         };
//         let row_id = cell.row_id();
//         let Some(record) = cursor.current_record(pager)? else {
//             return Ok(None);
//         };
//         let v = record.iter().map(|v| v.into_owned()).collect();
//         let row = Row::new(row_id, v);
//         cursor.save_position(pager)?;
//         Ok(Some(row))
//     }
// }

// #[derive(Debug)]
// pub struct SafeTableScan;
// impl<V: Vfs> RelationScan<V> for SafeTableScan {
//     fn next(
//         &mut self,
//         pager: &mut Pager<V>,
//         cursor: &mut BTreeCursor<V>,
//     ) -> Result<Option<Row>, SqliteError> {
//         if cursor.last_visited_entry().is_some() {
//             let row_id = cursor.with_current(pager, |_, c| Ok(c.row_id()))?;
//             let record = cursor.current_record(pager)?.unwrap();
//             let v = record.iter().map(|v| v.into_owned()).collect();
//             let row = Row::new(row_id, v);
//             cursor.next(pager)?;
//             return Ok(Some(row));
//         }
//         Ok(None)
//     }
// }

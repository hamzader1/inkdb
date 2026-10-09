use std::ops::Bound;

use crate::InkResult;
use crate::record::Value;
use crate::storage::btree::kind::HasRowId;
use crate::storage::btree::{BTreeCursor, TableLeaf};
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;
use super::scan_guard::ScanGuard;

/// Walks the rows of a table whose row ids fall in a range.
///
/// Row ids are the key of a table tree, so a range over them is walked in order
/// straight from the tree. An equality on the row id column turns into a range
/// with both ends the same.
#[derive(Debug)]
pub struct RowRangeScan<V: Vfs> {
    root_page: u32,
    rowid_column: Option<usize>,
    pub(crate) range: (Bound<i64>, Bound<i64>),
    cursor: BTreeCursor<V>,
    scan_guard: Box<dyn ScanGuard<V>>,
    is_init: bool,
    is_done: bool,
}

impl<V: Vfs> RowRangeScan<V> {
    pub fn new(
        root_page: u32,
        rowid_column: Option<usize>,
        start: Bound<i64>,
        end: Bound<i64>,
        scan_guard: Box<dyn ScanGuard<V>>,
    ) -> Self {
        Self {
            root_page,
            rowid_column,
            range: (start, end),
            cursor: BTreeCursor::new(root_page),
            scan_guard,
            is_init: false,
            is_done: false,
        }
    }

    /// The table root page.
    pub fn root_page(&self) -> u32 {
        self.root_page
    }

    /// The column that stands in for the row id, when the table has one.
    pub fn rowid_column(&self) -> Option<usize> {
        self.rowid_column
    }

    /// Whether a row id is inside the range.
    fn contains(&self, rowid: i64) -> bool {
        let above_start = match self.range.0 {
            Bound::Included(start) => rowid >= start,
            Bound::Excluded(start) => rowid > start,
            Bound::Unbounded => true,
        };
        let below_end = match self.range.1 {
            Bound::Included(end) => rowid <= end,
            Bound::Excluded(end) => rowid < end,
            Bound::Unbounded => true,
        };
        above_start && below_end
    }

    /// Hand on the next row inside the range.
    ///
    /// A range whose start is excluded seeks to that row id and then steps over
    /// any rows still on it, since a seek lands on the first row not less than
    /// the value. The walk ends as soon as a row id falls past the far end, since
    /// the rows come in order.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if !self.is_init {
            match self.range.0 {
                Bound::Included(start) => {
                    self.cursor
                        .seek_lower_bound(ctx.pager, &Value::Integer(start))?;
                }
                Bound::Excluded(start) => {
                    self.cursor
                        .seek_lower_bound(ctx.pager, &Value::Integer(start))?;
                    while let Some(cell) = self.cursor.current::<TableLeaf>(ctx.pager)?
                        && cell.row_id() as i64 <= start
                    {
                        self.cursor.next(ctx.pager)?;
                    }
                }
                Bound::Unbounded => {
                    self.cursor.first(ctx.pager)?;
                }
            }
            self.is_init = true;
        }
        self.scan_guard.restore(ctx.pager, &mut self.cursor)?;
        if self.is_done {
            return Ok(None);
        }
        let Some(cell) = self.cursor.current::<TableLeaf>(ctx.pager)? else {
            self.is_done = true;
            return Ok(None);
        };
        let rowid = cell.row_id() as i64;
        if !self.contains(rowid) {
            self.is_done = true;
            return Ok(None);
        }
        let Some(bytes) = self.cursor.current_record_bytes(ctx.pager)? else {
            self.is_done = true;
            return Ok(None);
        };
        let row = Row::stored_with_rowid(rowid as u64, bytes, self.rowid_column);
        self.scan_guard
            .save_or_advance(ctx.pager, &mut self.cursor)?;
        Ok(Some(row))
    }
}

/// Write a row id range the way a WHERE clause would, for an EXPLAIN.
pub fn render_rowid_range(range: &(Bound<i64>, Bound<i64>)) -> String {
    match (&range.0, &range.1) {
        (Bound::Included(start), Bound::Included(end)) if start == end => start.to_string(),
        _ => {
            let mut parts = Vec::new();
            match range.0 {
                Bound::Included(start) => parts.push(format!(">= {start}")),
                Bound::Excluded(start) => parts.push(format!("> {start}")),
                Bound::Unbounded => {}
            }
            match range.1 {
                Bound::Included(end) => parts.push(format!("<= {end}")),
                Bound::Excluded(end) => parts.push(format!("< {end}")),
                Bound::Unbounded => {}
            }
            parts.join(" AND ")
        }
    }
}

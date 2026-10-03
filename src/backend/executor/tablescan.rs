use crate::errors::InkError;
use crate::record::Record;
use crate::storage::btree::kind::HasPayload;
use crate::storage::btree::{BTreeCursor, TableLeaf};
use crate::storage::page::PageRef as BTreePageRef;
use crate::vfs::Vfs;

use super::context::ExecCtx;
use super::eval::Eval;
use super::scan_guard::{ScanGuard, ScanMode};
use super::{Row, RowView};

#[derive(Debug)]
pub struct TableScan<V: Vfs> {
    pub cursor: BTreeCursor<V>,
    pub guard: Box<dyn ScanGuard<V>>,
    pushed_predicate: Option<usize>,
    table_name: String,
    rowid_column: Option<usize>,
    rows_rejected: u64, /*used for testing*/
    is_init: bool,
    is_done: bool,
}
impl<V: Vfs> TableScan<V> {
    pub fn new(root_page: u32, mode: ScanMode, table_name: String) -> Result<Self, InkError> {
        let cursor = BTreeCursor::new(root_page);

        Ok(Self {
            cursor,
            is_done: false,
            guard: mode.guard(),
            pushed_predicate: None,
            table_name,
            rowid_column: None,
            is_init: false,
            rows_rejected: 0,
        })
    }

    pub fn set_pushed_predicate(&mut self, predicate: usize) {
        self.pushed_predicate = Some(predicate);
    }

    pub fn pushed_predicate(&self) -> Option<usize> {
        self.pushed_predicate
    }

    pub fn rows_rejected(&self) -> u64 {
        self.rows_rejected
    }
}
impl<V: Vfs> TableScan<V> {
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, InkError> {
        if !self.is_init {
            self.cursor.first(ctx.pager)?;
            let (page_no, _) = self.cursor.last_visited_entry_unchecked();
            let guard = ctx.pager.get(page_no)?;
            let page = BTreePageRef::new(
                page_no,
                ctx.pager.page_size(),
                ctx.pager.usable_size(),
                ctx.pager.header_len(),
                guard.bytes(),
            )?;
            let empty = page.no_of_cells()? == 0;
            if empty {
                self.is_done = true;
            }
            if let Some(table) = ctx.master.table(&self.table_name) {
                self.rowid_column = table.rowid_column();
            }
            self.is_init = true;
        }
        while !self.is_done {
            self.guard.restore(ctx.pager, &mut self.cursor)?;
            let Some(cell) = self.cursor.current::<TableLeaf>(ctx.pager)? else {
                self.is_done = true;
                return Ok(None);
            };
            let row_id = cell.row_id;
            let (page_no, _) = self.cursor.last_visited_entry_unchecked();
            let keep = match self.pushed_predicate {
                Some(predicate) => {
                    if cell.overflow_page().is_some() {
                        let Some(bytes) = self.cursor.current_record_bytes(ctx.pager)? else {
                            self.is_done = true;
                            return Ok(None);
                        };
                        let row = Row::stored_with_rowid(row_id, bytes, self.rowid_column);
                        Eval::eval(ctx.arena, predicate, Some(&row))?.to_bool()
                    } else {
                        let guard = ctx.pager.get(page_no)?;
                        let page = BTreePageRef::new(
                            page_no,
                            ctx.pager.page_size(),
                            ctx.pager.usable_size(),
                            ctx.pager.header_len(),
                            guard.bytes(),
                        )?;
                        let record = Record::new(&page.bytes()[cell.payload_range().clone()])?;
                        let view = RowView::new(row_id, record, self.rowid_column);
                        Eval::eval(ctx.arena, predicate, Some(&view))?.to_bool()
                    }
                }
                None => true,
            };

            if !keep {
                self.guard.save_or_advance(ctx.pager, &mut self.cursor)?;
                self.rows_rejected += 1;
                continue;
            }

            let Some(bytes) = self.cursor.current_record_bytes(ctx.pager)? else {
                self.is_done = true;
                return Ok(None);
            };
            self.guard.save_or_advance(ctx.pager, &mut self.cursor)?;
            return Ok(Some(Row::stored_with_rowid(
                row_id,
                bytes,
                self.rowid_column,
            )));
        }
        Ok(None)
    }
}

use crate::errors::SqliteError;
use crate::record::Value;
use crate::storage::btree::{BTreeCursor, TableLeaf};
use crate::storage::page::PageRef as BTreePageRef;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;
use super::eval::Eval;
use super::scan_guard::{ScanGuard, ScanMode};

#[derive(Debug)]
pub struct TableScan<V: Vfs> {
    pub cursor: BTreeCursor<V>,
    pub guard: Box<dyn ScanGuard<V>>,
    predicate: Option<usize>,
    table_name: String,
    rows_rejected: u64,
    is_init: bool,
    is_done: bool,
}
impl<V: Vfs> TableScan<V> {
    pub fn new(root_page: u32, mode: ScanMode, table_name: String) -> Result<Self, SqliteError> {
        let cursor = BTreeCursor::new(root_page);

        Ok(Self {
            cursor,
            is_done: false,
            guard: mode.guard(),
            predicate: None,
            table_name,
            is_init: false,
            rows_rejected: 0,
        })
    }

    pub fn set_predicate(&mut self, predicate: usize) {
        self.predicate = Some(predicate);
    }

    pub fn predicate(&self) -> Option<usize> {
        self.predicate
    }

    pub fn rows_rejected(&self) -> u64 {
        self.rows_rejected
    }
}
impl<V: Vfs> TableScan<V> {
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, SqliteError> {
        if !self.is_init {
            self.cursor.first(ctx.pager)?;
            let (page_no, _) = self.cursor.last_visited_entry_unchecked();
            let guard = ctx.pager.get(page_no)?;
            let page = BTreePageRef::new(
                page_no,
                ctx.pager.page_size(),
                ctx.pager.usable_size(),
                guard.bytes(),
            )?;
            let empty = page.no_of_cells()? == 0;
            if empty {
                self.is_done = true;
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

            let mut rejected = false;
            let values = match self.cursor.current_record::<TableLeaf>(ctx.pager)? {
                Some(mut record) => {
                    {
                        let Some(table) = ctx.master.table(&self.table_name) else {
                            unreachable!()
                        };
                        if let Some(idx) = table.has_integer_primary_key() {
                            record[idx] = row_id.into()
                        }
                    }

                    // let t = ctx.master.tables.get(k);
                    let keep = match self.predicate {
                        Some(predicate) => {
                            Eval::eval(ctx.arena, predicate, Some(&record))?.to_bool()
                        }
                        None => true,
                    };
                    if keep {
                        Some(record.into_iter().map(Value::into_static).collect())
                    } else {
                        rejected = true;
                        None
                    }
                }
                None => {
                    self.is_done = true;
                    None
                }
            };

            self.guard.save_or_advance(ctx.pager, &mut self.cursor)?;
            if rejected {
                self.rows_rejected += 1;
                continue;
            }
            if let Some(values) = values {
                return Ok(Some(Row::new(row_id, values)));
            }
        }
        Ok(None)
    }
}

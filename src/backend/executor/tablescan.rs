use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::sql::parser::ExprArena;
use crate::storage::btree::{BTreeCursor, RestorePosition, TableLeaf};
use crate::storage::page::PageRef as BTreePageRef;
use crate::vfs::Vfs;

use super::Row;
use super::eval::Eval;
use super::scan_guard::ScanGuard;

#[derive(Debug)]
pub struct TableScan<V: Vfs> {
    pub cursor: BTreeCursor<V>,
    is_done: bool,
    pub guard: Box<dyn ScanGuard<V>>,
    predicate: Option<usize>,
    rows_rejected: u64,
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
            predicate: None,
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
    pub fn next(
        &mut self,
        pager: &mut Pager<V>,
        arena: Option<&ExprArena>,
    ) -> Result<Option<Row>, SqliteError> {
        while !self.is_done {
            self.guard.restore(pager, &mut self.cursor)?;
            let Some(cell) = self.cursor.current::<TableLeaf>(pager)? else {
                self.is_done = true;
                return Ok(None);
            };
            let row_id = cell.row_id;

            let mut rejected = false;
            let values = match self.cursor.current_record::<TableLeaf>(pager)? {
                Some(record) => {
                    let keep = match (self.predicate, arena) {
                        (Some(predicate), Some(arena)) => {
                            Eval::eval(arena, predicate, Some(&record))?.to_bool()
                        }
                        _ => true,
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

            self.guard.save_or_advance(pager, &mut self.cursor)?;
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

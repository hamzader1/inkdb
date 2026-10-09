use std::ops::{Bound, RangeBounds};

use crate::InkResult;
use crate::record::Record;
use crate::{
    backend::{
        analyzer::{IndexMetadata, rowid_of},
        executor::Row,
        planner::plan::Plan,
    },
    errors::CorruptError,
    errors::InkError,
    record::{Value, tuple::Tuple},
    storage::{
        btree::{BTree, BTreeCursor, IndexLeaf, SeekResult},
        cell::Encode,
    },
    vfs::Vfs,
};

use super::{context::ExecCtx, scan_guard::ScanGuard};
/// Runs one index change for each row its child yields.
///
/// The entry is built from the indexed column and the row id, and handed to
/// whichever index action this was built with, an insert or a delete.
#[derive(Debug)]
pub struct PrepareIndex<V: Vfs> {
    index: IndexMetadata,
    action: Box<dyn IndexMutation<V>>,
    child: Box<Plan<V>>,
}

impl<V: Vfs> PrepareIndex<V> {
    pub(crate) fn new(
        index: IndexMetadata,
        action: Box<dyn IndexMutation<V>>,
        child: Box<Plan<V>>,
    ) -> Self {
        Self {
            index,
            action,
            child,
        }
    }
    /// Change the index for one row from below, then pass the row on.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        let Some(row) = self.child.next(ctx)? else {
            return Ok(None);
        };
        let value = row.value(self.index.col_idx)?.into_static();
        let key = self.index.key_for(value, row.key());
        let mut btree = BTree::new(self.index.index_root_page, ctx.pager);

        self.action.next(&mut btree, &key)?;
        Ok(Some(row))
    }

    /// The operator whose rows are indexed.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    /// The index root page.
    pub fn index_root_page(&self) -> u32 {
        self.index.index_root_page
    }
    /// The table column the index covers.
    pub fn col_idx(&self) -> usize {
        self.index.col_idx
    }
    /// The name of the action, for an EXPLAIN.
    pub fn action_name(&self) -> String {
        format!("{:?}", self.action)
    }
}

/// Finds the rows whose indexed column equals one value.
///
/// The index entry holds the value and the row id, so a single value can match
/// several rows; the scan walks entries while the value stays the same, and looks
/// each row id up in the table as it goes.
#[derive(Debug)]
pub struct IndexExactMatch<V: Vfs> {
    index_root_page: u32,
    relation_root_page: u32,
    relation_name: String,
    target: Value<'static>,
    cursor: BTreeCursor<V>,
    scan_guard: Box<dyn ScanGuard<V>>,
    is_init: bool,
    is_done: bool,
}

impl<V: Vfs> IndexExactMatch<V> {
    pub fn new(
        index_root_page: u32,
        relation_root_page: u32,
        relation_name: String,
        target: Value<'static>,
        scan_guard: Box<dyn ScanGuard<V>>,
    ) -> Result<Self, InkError> {
        let cursor = BTreeCursor::new(index_root_page);
        Ok(Self {
            index_root_page,
            relation_root_page,
            relation_name,
            target,
            cursor,
            scan_guard,
            is_init: false,
            is_done: false,
        })
    }
    /// The index root page.
    pub fn index_root_page(&self) -> u32 {
        self.index_root_page
    }
    /// The table root page the row ids point into.
    pub fn relation_root_page(&self) -> u32 {
        self.relation_root_page
    }
    /// The value being looked for.
    pub fn target(&self) -> &Value<'_> {
        &self.target
    }
    /// Hand on the next row with this value. The walk stops as soon as an entry no
    /// longer equals it, since the entries are in value order.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if !self.is_init {
            self.cursor
                .seek_lower_bound(ctx.pager, &Value::Tuple([self.target.clone()].into()))?;
            self.is_init = true;
        }

        // If the previous row survived, step over it. If it was deleted,
        // restore already sits on its successor.
        self.scan_guard.restore(ctx.pager, &mut self.cursor)?;

        if self.is_done {
            return Ok(None);
        }

        let Some(index_bytes) = self.cursor.current_record_bytes(ctx.pager)? else {
            self.is_done = true;
            return Ok(None);
        };
        let index_record = Record::new(&index_bytes)?;

        if index_record.value(0)? != self.target {
            self.is_done = true;
            return Ok(None);
        }
        let row_id = rowid_of(&index_record)?;

        let pk_as_rowid = {
            match ctx.master.table(&self.relation_name) {
                Some(table) => table.rowid_column(),
                _ => None,
            }
        };
        let mut relation_btree = BTree::new(self.relation_root_page, ctx.pager);
        if relation_btree.seek(&Value::Integer(row_id as i64))? != SeekResult::Exact {
            return Err(CorruptError::IndexEntryWithoutRow {
                index_page: self.index_root_page,
                rowid: row_id,
                table_page: self.relation_root_page,
            }
            .into());
        }
        let relation_record = relation_btree
            .cursor
            .current_record_bytes(ctx.pager)?
            .ok_or(InkError::Corrupt(CorruptError::RowVanished))?;

        let row = Row::stored_with_rowid(row_id, relation_record, pk_as_rowid);
        self.scan_guard
            .save_or_advance(ctx.pager, &mut self.cursor)?;

        Ok(Some(row))
    }
}
/// One change to an index, run once per row.
pub trait IndexMutation<V: Vfs>: std::fmt::Debug {
    fn next(&mut self, btree: &mut BTree<V>, entry: &[Value]) -> InkResult<()>;
}

/// Removes an index entry.
#[derive(Debug)]
pub struct IndexDelete;
impl<V: Vfs> IndexMutation<V> for IndexDelete {
    fn next(&mut self, btree: &mut BTree<V>, entry: &[Value]) -> InkResult<()> {
        if !btree.delete(Value::Tuple(entry.into()))? {
            return Err(CorruptError::IndexEntryMissing {
                index_page: btree.root_page,
            }
            .into());
        }
        Ok(())
    }
}
/// Adds an index entry, checking for a clash first when the index is unique.
///
/// The check looks for the value without the row id, since two rows with the same
/// value are what a unique index exists to reject. A NULL is not a clash, so
/// several rows may hold NULL in a unique column.
#[derive(Debug)]
pub struct IndexInsert {
    pub(crate) is_unique: bool,
}
impl<V: Vfs> IndexMutation<V> for IndexInsert {
    fn next(&mut self, btree: &mut BTree<V>, entry: &[Value]) -> InkResult<()> {
        if self.is_unique {
            btree.seek(&Value::Tuple([entry[0].clone()].into()))?;
            if let Some(record) = btree.current_record::<IndexLeaf>()?
                && !record[0].is_null()
                && record[0] == entry[0]
            {
                return Err(InkError::runtime(format!(
                    "violates unique index constraint for value: {}",
                    entry[0]
                )));
            }
        }

        let bytes = Encode::encode_index_leaf_cell(Tuple::serialize(entry));
        btree.insert(&Value::Tuple(entry.into()), bytes)?;
        Ok(())
    }
}

/// Walks the index entries whose value falls in a range.
///
/// The entries come in value order, which makes a range an easy walk: it ends as
/// soon as a value passes the far end. Each entry carries its row id, which is
/// used to fetch the row from the table.
#[derive(Debug)]
pub struct IndexRangeScan<V: Vfs> {
    index_root_page: u32,
    relation_root_page: u32,
    pub(crate) range: (Bound<Value<'static>>, Bound<Value<'static>>),
    pub(crate) scan_guard: Box<dyn ScanGuard<V>>,
    cursor: BTreeCursor<V>,
    is_init: bool,
    is_done: bool,
}

impl<V: Vfs> IndexRangeScan<V> {
    pub fn new(
        index_root_page: u32,
        relation_root_page: u32,
        start: Bound<Value<'static>>,
        end: Bound<Value<'static>>,
        scan_guard: Box<dyn ScanGuard<V>>,
    ) -> InkResult<Self> {
        assert!(!(matches!(start, Bound::Unbounded) && matches!(end, Bound::Unbounded)));
        let cursor = BTreeCursor::<V>::new(index_root_page);

        Ok(Self {
            index_root_page,
            relation_root_page,
            range: (start, end),
            scan_guard,
            cursor,
            is_init: false,
            is_done: false,
        })
    }

    /// The index root page.
    pub fn index_root_page(&self) -> u32 {
        self.index_root_page
    }
    /// The table root page the row ids point into.
    pub fn relation_root_page(&self) -> u32 {
        self.relation_root_page
    }
    /// The range of values being walked.
    pub fn range(&self) -> &(Bound<Value<'static>>, Bound<Value<'static>>) {
        &self.range
    }

    /// Hand on the next row whose indexed value is inside the range. A row id with
    /// no row behind it is a corrupt index, and is reported rather than skipped.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if !self.is_init {
            match self.range.0 {
                Bound::Included(ref i) => {
                    self.cursor
                        .seek_lower_bound(ctx.pager, &Value::Tuple([i.to_owned_static()].into()))?;
                }
                Bound::Excluded(ref i) => {
                    self.cursor
                        .seek_lower_bound(ctx.pager, &Value::Tuple([i.to_owned_static()].into()))?;
                    while let Some(bytes) = self.cursor.current_record_bytes(ctx.pager)?
                        && Record::new(&bytes)?.value(0)? == *i
                    {
                        self.cursor.next(ctx.pager)?;
                    }
                }
                _ => {
                    self.cursor.first(ctx.pager)?;
                }
            };
            self.is_init = true;
        }
        self.scan_guard.restore(ctx.pager, &mut self.cursor)?;
        if self.is_done {
            return Ok(None);
        }

        let Some(index_bytes) = self.cursor.current_record_bytes(ctx.pager)? else {
            self.is_done = true;
            return Ok(None);
        };
        let index_record = Record::new(&index_bytes)?;
        if !self.range.contains(&index_record.value(0)?) {
            self.is_done = true;
            return Ok(None);
        }
        let row_id = rowid_of(&index_record)?;
        let mut relation_btree = BTree::new(self.relation_root_page, ctx.pager);
        if relation_btree.seek(&Value::Integer(row_id as i64))? != SeekResult::Exact {
            return Err(CorruptError::IndexEntryWithoutRow {
                index_page: self.index_root_page,
                rowid: row_id,
                table_page: self.relation_root_page,
            }
            .into());
        }
        let relation_record = relation_btree
            .cursor
            .current_record_bytes(ctx.pager)?
            .ok_or(InkError::Corrupt(CorruptError::RowVanished))?;

        let row = Row::stored(row_id, relation_record);
        self.scan_guard
            .save_or_advance(ctx.pager, &mut self.cursor)?;
        Ok(Some(row))
    }
}

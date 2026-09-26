use std::ops::{Bound, RangeBounds};

use crate::SqliteResult;
use crate::{
    backend::{
        analyze::{IndexMetadata, rowid_of},
        executor::Row,
        planner::plan::Plan,
    },
    errors::CorruptError,
    errors::SqliteError,
    record::{Value, tuple::Tuple},
    storage::{
        btree::{BTree, BTreeCursor, IndexLeaf, SeekResult, TableLeaf},
        cell::Encode,
    },
    vfs::Vfs,
};

use super::{context::ExecCtx, scan_guard::ScanGuard};
#[derive(Debug)]
pub struct PrepareIndex<V: Vfs> {
    index: IndexMetadata,
    action: Box<dyn IndexMutation<V>>,
    child: Box<Plan<V>>,
}

impl<V: Vfs> PrepareIndex<V> {
    pub fn new(
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
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        let Some(row) = self.child.next(ctx)? else {
            return Ok(None);
        };
        let key = self.index.key_for(&row, row.key());
        let mut btree = BTree::new(self.index.index_root_page, ctx.pager);

        self.action.next(&mut btree, &key)?;
        Ok(Some(row))
    }

    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    pub fn index_root_page(&self) -> u32 {
        self.index.index_root_page
    }
    pub fn col_idx(&self) -> usize {
        self.index.col_idx
    }
    pub fn action_name(&self) -> String {
        format!("{:?}", self.action)
    }
}

#[derive(Debug)]
pub struct IndexExactMatch<V: Vfs> {
    index_root_page: u32,
    relation_root_page: u32,
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
        target: Value<'static>,
        scan_guard: Box<dyn ScanGuard<V>>,
    ) -> Result<Self, SqliteError> {
        let cursor = BTreeCursor::new(index_root_page);
        Ok(Self {
            index_root_page,
            relation_root_page,
            target,
            cursor,
            scan_guard,
            is_init: false,
            is_done: false,
        })
    }
    pub fn index_root_page(&self) -> u32 {
        self.index_root_page
    }
    pub fn relation_root_page(&self) -> u32 {
        self.relation_root_page
    }
    pub fn target(&self) -> &Value<'_> {
        &self.target
    }
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        if !self.is_init {
            self.cursor
                .seek_lower_bound(ctx.pager, &Value::Tuple(vec![self.target.clone()]))?;
            self.is_init = true;
        }

        // If the previous row survived, step over it. If it was deleted,
        // restore already sits on its successor.
        self.scan_guard.restore(ctx.pager, &mut self.cursor)?;

        if self.is_done {
            return Ok(None);
        }

        let Some(index_record) = self.cursor.current_record::<IndexLeaf>(ctx.pager)? else {
            self.is_done = true;
            return Ok(None);
        };

        if index_record[0] != self.target {
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
            .current_record::<TableLeaf>(ctx.pager)?
            .ok_or(SqliteError::Corrupt(CorruptError::RowVanished))?
            .iter()
            .map(|v| v.to_owned_static())
            .collect();

        let row = Row::new(row_id, relation_record);
        self.scan_guard
            .save_or_advance(ctx.pager, &mut self.cursor)?;
        // self.cursor.save_position(pager)?;

        Ok(Some(row))
    }
}
pub trait IndexMutation<V: Vfs>: std::fmt::Debug {
    fn next(&mut self, btree: &mut BTree<V>, entry: &[Value]) -> SqliteResult<()>;
}

#[derive(Debug)]
pub struct IndexDelete;
impl<V: Vfs> IndexMutation<V> for IndexDelete {
    fn next(&mut self, btree: &mut BTree<V>, entry: &[Value]) -> SqliteResult<()> {
        if !btree.delete(Value::Tuple(entry.to_vec()))? {
            return Err(CorruptError::IndexEntryMissing {
                index_page: btree.root_page,
            }
            .into());
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct IndexInsert {
    pub is_unique: bool,
}
impl<V: Vfs> IndexMutation<V> for IndexInsert {
    fn next(&mut self, btree: &mut BTree<V>, entry: &[Value]) -> SqliteResult<()> {
        btree.seek(&Value::Tuple(entry.to_vec()))?;
        if self.is_unique
            && let Some(record) = btree.current_record::<IndexLeaf>()?
            && record[0] == entry[0]
        {
            return Err(SqliteError::runtime(format!(
                "violates unique index constraint for value: {}",
                entry[0]
            )));
        }

        let mut bytes = Encode::encode_index_leaf_cell(Tuple::serialize(entry));
        btree.insert(&Value::Tuple(entry.to_vec()), &mut bytes)?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct IndexRangeScan<V: Vfs> {
    index_root_page: u32,
    relation_root_page: u32,
    pub range: (Bound<Value<'static>>, Bound<Value<'static>>),
    pub scan_guard: Box<dyn ScanGuard<V>>,
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
    ) -> SqliteResult<Self> {
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

    pub fn index_root_page(&self) -> u32 {
        self.index_root_page
    }
    pub fn relation_root_page(&self) -> u32 {
        self.relation_root_page
    }
    pub fn range(&self) -> &(Bound<Value<'static>>, Bound<Value<'static>>) {
        &self.range
    }

    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        if !self.is_init {
            match self.range.0 {
                Bound::Included(ref i) => {
                    self.cursor
                        .seek_lower_bound(ctx.pager, &Value::Tuple(vec![i.to_owned_static()]))?;
                }
                Bound::Excluded(ref i) => {
                    self.cursor
                        .seek_lower_bound(ctx.pager, &Value::Tuple(vec![i.to_owned_static()]))?;
                    while let Some(record) = self.cursor.current_record::<IndexLeaf>(ctx.pager)?
                        && &record[0] == i
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

        let Some(index_record) = self.cursor.current_record::<IndexLeaf>(ctx.pager)? else {
            self.is_done = true;
            return Ok(None);
        };
        if !self.range.contains(&index_record[0]) {
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
            .current_record::<TableLeaf>(ctx.pager)?
            .ok_or(SqliteError::Corrupt(CorruptError::RowVanished))?
            .iter()
            .map(|v| v.to_owned_static())
            .collect();

        let row = Row::new(row_id, relation_record);
        self.scan_guard
            .save_or_advance(ctx.pager, &mut self.cursor)?;
        Ok(Some(row))
    }
}

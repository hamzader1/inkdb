use crate::{
    SqliteResult,
    backend::{
        executor::{Row, eval::Eval},
        planner::plan::{Plan, Terminate},
    },
    errors::SqliteError,
    pager::pager::Pager,
    record::{Value, tuple::Tuple},
    sql::parser::ExprArena,
    storage::{
        btree::{BTree, BTreeCursor, RestorePosition, SeekResult},
        cell::Encode,
    },
    vfs::file::SqliteFile,
};

use super::{insert::Insert, scan_guard::ScanGuard};
#[derive(Debug)]
pub struct PrepareIndex<F: SqliteFile> {
    index_root_page: u32,
    col_idx: usize,
    action: Box<dyn IndexMutation<F>>,
    child: Box<Plan<F>>,
}

impl<F: SqliteFile> PrepareIndex<F> {
    pub fn new(
        index_root_page: u32,
        col_idx: usize,
        action: Box<dyn IndexMutation<F>>,
        child: Box<Plan<F>>,
    ) -> Self {
        Self {
            index_root_page,
            col_idx,
            action,
            child,
        }
    }
    pub fn next(
        &mut self,
        pager: &mut Pager<F>,
        arena: Option<&ExprArena>,
    ) -> SqliteResult<Option<Row>> {
        let Some(row) = self.child.next(pager, arena)? else {
            return Ok(None);
        };
        let key = vec![row[self.col_idx].clone(), Value::Integer(row.key as _)];
        let mut btree = BTree::new(self.index_root_page, pager);

        self.action.next(&mut btree, key)?;
        Ok(Some(row))
    }

    /// Child subtree for optimizer traversal.
    pub fn child_mut(&mut self) -> &mut Plan<F> {
        &mut self.child
    }
}

#[derive(Debug)]
pub struct IndexExactMatch<F: SqliteFile> {
    index_root_page: u32,
    relation_root_page: u32,
    target: Value<'static>,
    cursor: BTreeCursor<F>,
    scan_guard: Box<dyn ScanGuard<F>>,
    is_done: bool,
}

impl<F: SqliteFile> IndexExactMatch<F> {
    pub fn new(
        pager: &mut Pager<F>,
        index_root_page: u32,
        relation_root_page: u32,
        target: Value<'static>,
        scan_guard: Box<dyn ScanGuard<F>>,
    ) -> Result<Self, SqliteError> {
        // Park on the first entry at or after the wanted key. Matches may
        // live in a later leaf than the raw landing, so done stays false
        // here and the key check in next decides when the run ends.
        let mut cursor = BTreeCursor::new(index_root_page);
        cursor.seek_lower_bound(pager, &Value::Tuple(vec![target.clone()]))?;
        Ok(Self {
            index_root_page,
            relation_root_page,
            target,
            cursor,
            scan_guard,
            is_done: false,
        })
    }
    pub fn next(&mut self, pager: &mut Pager<F>, arena: &ExprArena) -> SqliteResult<Option<Row>> {
        // If the previous row survived, step over it. If it was deleted,
        // restore already sits on its successor.
        self.scan_guard.restore(pager, &mut self.cursor)?;

        if self.is_done {
            return Ok(None);
        }

        let Some(mut index_record) = self.cursor.current_record(pager)? else {
            self.is_done = true;
            return Ok(None);
        };

        let row_id = index_record
            .pop()
            .expect("Index record is empty")
            .into_owned();

        if index_record[0] != self.target {
            self.is_done = true;
            return Ok(None);
        }

        // Every index entry must point at a live table row. A missing row
        // means the table delete and the index delete disagreed, so speak
        // up instead of returning a wrong row.
        let mut relation_btree = BTree::new(self.relation_root_page, pager);
        if relation_btree.seek(&row_id)? != SeekResult::Exact {
            return Err(SqliteError::Corrupt(format!(
                "index {} holds rowid {row_id} but table {} has no such row",
                self.index_root_page, self.relation_root_page
            )));
        }
        let relation_record = relation_btree
            .cursor
            .current_record(pager)?
            .ok_or_else(|| SqliteError::Corrupt("row vanished between exact seek and read".into()))?
            .iter()
            .map(|v| v.into_owned())
            .collect();

        let row = Row::new(row_id.cast_int()? as _, relation_record);
        self.scan_guard.save_or_advance(pager, &mut self.cursor)?;
        // self.cursor.save_position(pager)?;

        Ok(Some(row))
    }
}
pub trait IndexMutation<F: SqliteFile>: std::fmt::Debug {
    fn next(&mut self, btree: &mut BTree<F>, key: Vec<Value>) -> SqliteResult<()>;
}

#[derive(Debug)]
pub struct IndexDelete;
impl<F: SqliteFile> IndexMutation<F> for IndexDelete {
    fn next(&mut self, btree: &mut BTree<F>, key: Vec<Value>) -> SqliteResult<()> {
        // A missing entry for a row being deleted is corruption. Silently
        // ignoring it is how rows survived DELETE while the index lost
        // track of them.
        if !btree.delete(Value::Tuple(key))? {
            return Err(SqliteError::Corrupt(format!(
                "index root {}: entry missing for a row being deleted",
                btree.root_page
            )));
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct IndexInsert {
    pub is_unique: bool,
}
impl<F: SqliteFile> IndexMutation<F> for IndexInsert {
    fn next(&mut self, btree: &mut BTree<F>, key: Vec<Value>) -> SqliteResult<()> {
        btree.seek(&Value::Tuple(key.clone()))?;
        if self.is_unique
            && let Some(record) = btree.current_record()?
            && record[0] == key[0]
        {
            return Err(SqliteError::Runtime(format!(
                "violates unique index constraint for value: {}",
                key[0]
            )));
        }

        let mut bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&key));
        Insert::<'_, F>::new(btree.root_page, Value::Tuple(key), &mut bytes).next(btree.pager)?;
        Ok(())
    }
}

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
        btree::{BTree, BTreeCursor, SeekResult},
        cell::Encode,
    },
    vfs::file::SqliteFile,
};

use super::insert::Insert;

#[derive(Debug)]
pub struct PrepareIndex<F: SqliteFile> {
    index_root_page: u32,
    col_idx: usize,
    is_unique: bool,
    child: Box<Plan<F>>,
}

impl<F: SqliteFile> PrepareIndex<F> {
    pub fn new(index_root_page: u32, col_idx: usize, is_unique: bool, child: Box<Plan<F>>) -> Self {
        Self {
            index_root_page,
            col_idx,
            is_unique,
            child,
        }
    }

    /// Child subtree for optimizer traversal.
    pub fn child_mut(&mut self) -> &mut Plan<F> {
        &mut self.child
    }

    pub fn next(&mut self, pager: &mut Pager<F>) -> SqliteResult<Option<Row>> {
        let Some(row) = self.child.next(pager, None)? else {
            return Ok(None);
        };
        let key = vec![row[self.col_idx].clone(), Value::Integer(row.key as _)];
        let mut btree = BTree::new(self.index_root_page, pager);

        /*
         * Insert path
         */
        btree.seek(&Value::Tuple(vec![row[self.col_idx].clone()]))?;
        if self.is_unique
            && let Some(record) = btree.current_record()?
            && record[0] == key[0]
        {
            return Err(SqliteError::Runtime(format!(
                "violates unique index constraint for value: {}",
                row[self.col_idx]
            )));
        }
        let mut bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&key));
        Insert::<'_, F>::new(self.index_root_page, Value::Tuple(key), &mut bytes).next(pager)?;
        Ok(Some(row))
    }
}

#[derive(Debug)]
pub struct IndexExactMatch<F: SqliteFile> {
    // child: Box<Plan<F>>,
    index_root_page: u32,
    relation_root_page: u32,
    target: Value<'static>,
    cursor: BTreeCursor<F>,
    is_done: bool,
}

impl<F: SqliteFile> IndexExactMatch<F> {
    pub fn new(
        pager: &mut Pager<F>,
        index_root_page: u32,
        relation_root_page: u32,
        target: Value<'static>,
    ) -> Result<Self, SqliteError> {
        let mut cursor = BTreeCursor::new(index_root_page);
        let seek_res = cursor.seek(pager, &Value::Tuple(vec![target.clone()]))?;
        if seek_res == SeekResult::Exact
            && let Some(p) = cursor.stack.last_mut()
        {
            p.yeilded = true;
        }
        Ok(Self {
            index_root_page,
            relation_root_page,
            target,
            cursor,
            is_done: seek_res == SeekResult::NotFound,
        })
    }
    pub fn next(&mut self, pager: &mut Pager<F>, arena: &ExprArena) -> SqliteResult<Option<Row>> {
        if self.is_done {
            return Ok(None);
        }
        let current_index_record = self.cursor.current_record(pager)?;
        if let Some(mut index_record) = current_index_record {
            let row_id = index_record.pop().expect("Index record is empty");
            if index_record != vec![self.target.clone()] {
                self.is_done = true;
                return Ok(None);
            }
            let mut relation_btree = BTree::new(self.relation_root_page, pager);
            relation_btree.seek(&row_id.clone());
            let relation_record = relation_btree
                .cursor
                .current_record(pager)?
                .expect("Row id not associated with any record")
                .iter()
                .map(|v| v.into_owned())
                .collect();

            let row = Row::new(row_id.cast_int()? as _, relation_record);
            self.cursor.next(pager)?;
            return Ok(Some(row));
        }
        self.is_done = true;
        Ok(None)
    }
}

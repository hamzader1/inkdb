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

use super::insert::Insert;

// #[derive(Debug)]
// pub struct PrepareIndex<F: SqliteFile> {
//     index_root_page: u32,
//     col_idx: usize,
//     is_unique: bool,
//     child: Box<Plan<F>>,
// }
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
        // println!("KEY TO BE DELETED {:?} ", key);

        self.action.next(&mut btree, key)?;
        /*
         * Insert path
         */
        // let alpha = vec![key[0].clone()];
        // btree.seek(&Value::Tuple(alpha))?;
        // if self.is_unique
        //     && let Some(record) = btree.current_record()?
        //     && record[0] == key[0]
        // {
        //     return Err(SqliteError::Runtime(format!(
        //         "violates unique index constraint for value: {}",
        //         row[self.col_idx]
        //     )));
        // }
        // let mut bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&key));
        // Insert::<'_, F>::new(self.index_root_page, Value::Tuple(key), &mut bytes).next(pager)?;
        /*
         *
         */
        Ok(Some(row))
    }

    /// Child subtree for optimizer traversal.
    pub fn child_mut(&mut self) -> &mut Plan<F> {
        &mut self.child
    }
}

// impl<F: SqliteFile> PrepareIndex<F> {
//     pub fn new(index_root_page: u32, col_idx: usize, is_unique: bool, child: Box<Plan<F>>) -> Self {
//         Self {
//             index_root_page,
//             col_idx,
//             is_unique,
//             child,
//         }
//     }

//     /// Child subtree for optimizer traversal.
//     pub fn child_mut(&mut self) -> &mut Plan<F> {
//         &mut self.child
//     }

//     pub fn next(&mut self, pager: &mut Pager<F>) -> SqliteResult<Option<Row>> {
//         let Some(row) = self.child.next(pager, None)? else {
//             return Ok(None);
//         };
//         let key = vec![row[self.col_idx].clone(), Value::Integer(row.key as _)];
//         let mut btree = BTree::new(self.index_root_page, pager);

//         /*
//          * Insert path
//          */
//         let alpha = vec![key[0].clone()];
//         btree.seek(&Value::Tuple(alpha))?;
//         if self.is_unique
//             && let Some(record) = btree.current_record()?
//             && record[0] == key[0]
//         {
//             return Err(SqliteError::Runtime(format!(
//                 "violates unique index constraint for value: {}",
//                 row[self.col_idx]
//             )));
//         }
//         let mut bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&key));
//         Insert::<'_, F>::new(self.index_root_page, Value::Tuple(key), &mut bytes).next(pager)?;
//         /*
//          *
//          */
//         Ok(Some(row))
//     }
// }

#[derive(Debug)]
pub struct IndexExactMatch<F: SqliteFile> {
    index_root_page: u32,
    relation_root_page: u32,
    target: Value<'static>,
    cursor: BTreeCursor<F>,
    cnt: usize,
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
            cnt: 0,
            is_done: seek_res == SeekResult::NotFound,
        })
    }
    pub fn next(&mut self, pager: &mut Pager<F>, arena: &ExprArena) -> SqliteResult<Option<Row>> {
        self.cnt += 1;

        match self.cursor.restore_position(pager)? {
            RestorePosition::Exact => {
                self.cursor.next(pager)?;
            }
            RestorePosition::Next => {}
            RestorePosition::Empty => {}
        }

        if self.is_done {
            return Ok(None);
        }

        let current_index_record = self.cursor.current_record(pager)?;
        let Some(mut index_record) = current_index_record else {
            eprintln!(
                "IndexExactMatch DONE current_record None stack {:?}",
                self.cursor.stack
            );
            self.is_done = true;
            return Ok(None);
        };

        let row_id = index_record
            .pop()
            .expect("Index record is empty")
            .into_owned();

        if index_record[0] != self.target {
            eprintln!(
                "IndexExactMatch DONE mismatch: got {:?} target {:?}",
                index_record[0], self.target
            );
            eprintln!("stack {:?}", self.cursor.stack);
            self.is_done = true;
            return Ok(None);
        }

        let mut relation_btree = BTree::new(self.relation_root_page, pager);
        relation_btree.seek(&row_id);
        let relation_record = relation_btree
            .cursor
            .current_record(pager)?
            .expect("Row id not associated with any record")
            .iter()
            .map(|v| v.into_owned())
            .collect();

        let row = Row::new(row_id.cast_int()? as _, relation_record);
        self.cursor.save_position(pager)?;

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
        btree.delete(Value::Tuple(key))?;
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

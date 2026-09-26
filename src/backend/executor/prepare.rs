use crate::record::tuple::Tuple;
use crate::storage::btree::BTree;
use crate::storage::cell::Encode;
use crate::{backend::planner::plan::Plan, record::Value, vfs::Vfs};

/*
 *
 * PrepareRow does the following
 * Taking a Pre validated Row and seeking into the right position
 * where it should be, as well as validating *table constraits
 *
 */
#[derive(Debug)]
pub struct PrepareRow<V: Vfs> {
    #[allow(dead_code)]
    child: Option<Box<Plan<V>>>,
    pub root_page: u32,
    pub rows: Vec<Vec<Value<'static>>>,
    pub table_constraints: Option<Vec<usize>>,
    pos: usize,
}

impl<V: Vfs> PrepareRow<V> {
    pub fn new(
        child: Option<Box<Plan<V>>>,
        root_page: u32,
        rows: Vec<Vec<Value<'static>>>,
        table_constraints: Option<Vec<usize>>,
    ) -> Self {
        Self {
            child,
            root_page,
            rows,
            table_constraints,
            pos: 0,
        }
    }

    pub fn next(&mut self, pager: &mut Pager<V>) -> Result<Option<Row>, SqliteError> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let mut btree = BTree::new(self.root_page, pager);
        let next_row_id = btree.cursor.max_row_id(pager)? + 1;
        let inner = &self.rows[self.pos];
        let mut bytes = Encode::encode_table_leaf_cell(Tuple::serialize(inner), next_row_id as _);
        /*
         * insert here
         */
        Insert::new(self.root_page, next_row_id.into(), bytes).next(pager)?;
        let out = Row::new(
            next_row_id,
            inner.iter().map(|v| v.to_owned_static()).collect(),
        );
        self.pos += 1;
        Ok(Some(out))
    }
}
use crate::backend::executor::Row;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;

use super::insert::Insert;

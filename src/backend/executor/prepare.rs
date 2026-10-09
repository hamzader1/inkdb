use std::marker::PhantomData;

use crate::InkResult;
use crate::record::tuple::Tuple;
use crate::storage::btree::{BTree, TableLeaf};
use crate::storage::cell::Encode;
use crate::{backend::planner::plan::Plan, record::Value, vfs::Vfs};

/// Turns a row into a table cell and puts it in the tree.
///
/// A row that names its own primary key is checked against the rows already
/// there, and otherwise the next free row id is taken from the largest one in the
/// tree. The key column is blanked before storing, since a key that aliases the
/// row id is not kept in the row as well.
#[derive(Debug)]
pub struct PrepareRow<V: Vfs> {
    child: Box<Plan<V>>,
    pub(crate) root_page: u32,
    table_name: String,
}

impl<V: Vfs> PrepareRow<V> {
    pub fn new(child: Box<Plan<V>>, root_page: u32, table_name: String) -> Self {
        Self {
            child,
            root_page,
            table_name,
        }
    }
    /// The operator this one pulls from.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }

    /// Store one row and hand it on.
    ///
    /// The row comes back with the row id it was stored under, which the index
    /// operators above need in order to build their entries.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, InkError> {
        let row = {
            let Some(row) = self.child.next(ctx)? else {
                return Ok(None);
            };
            row
        };
        let inner = &mut row.to_values()?;
        let mut btree = BTree::new(self.root_page, ctx.pager);
        let mut next_row_id = btree.max_row_id()? + 1;
        /*
         * Check if we are doing violition or not
         */
        if self.root_page != 1
            && let Some(t) = ctx.master.table(&self.table_name)
            && let Some(idx) = t.rowid_column()
            && !inner[idx].is_null()
        {
            btree.seek(&inner[idx])?;

            let is_duplicated = btree
                .current_cell::<TableLeaf>()?
                .is_some_and(|x| Value::Integer(x.row_id as _) == inner[idx]);
            if is_duplicated {
                return Err(InkError::runtime(format!(
                    "Unique UNIQUE constraint failed on {}.{}",
                    t.name(),
                    t.get_col_name(idx).unwrap().name
                )));
            }
            next_row_id = inner[idx].cast_int()? as _;
            // This is a special case in SQLite.
            // If the table has an INTEGER PRIMARY KEY, its value is the same as the
            // rowid stored as the key in the B tree, so we store NULL for that column.
            // When retrieving the row, we do the reverse and replace the INTEGER PRIMARY
            // KEY column with the rowid.
            inner[idx] = Value::Null;
        }

        let bytes = Encode::encode_table_leaf_cell(Tuple::serialize(inner), next_row_id as _);
        /*
         * insert here
         */
        Insert::new(self.root_page, next_row_id.into(), bytes).next(ctx)?;
        let out = Row::new(
            next_row_id,
            inner.iter().map(|v| v.to_owned_static()).collect(),
        );
        Ok(Some(out))
    }
}
use crate::backend::executor::Row;
use crate::errors::InkError;

use super::context::ExecCtx;
use super::insert::Insert;

/// Hands out the rows of an INSERT, one VALUES tuple at a time.
///
/// The values were all worked out when the statement was resolved, so this only
/// has to hand them on, one row per call.
#[derive(Debug)]
pub struct PrepareInsert<V: Vfs> {
    pub(crate) rows: Vec<Vec<Value<'static>>>,
    pos: usize,
    _marker: PhantomData<fn() -> V>,
}

impl<V: Vfs> PrepareInsert<V> {
    pub fn new(rows: Vec<Vec<Value<'static>>>) -> Self {
        Self {
            rows,
            pos: 0,
            _marker: PhantomData,
        }
    }
    /// Hand out the next tuple of values as a row.
    pub fn next(&mut self, _ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let values = std::mem::take(&mut self.rows[self.pos]);
        self.pos += 1;
        Ok(Some(Row::new(0, values.into_iter().collect())))
    }
}

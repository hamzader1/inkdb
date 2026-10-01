use std::marker::PhantomData;

use crate::InkResult;
use crate::record::tuple::Tuple;
use crate::storage::btree::{BTree, TableLeaf};
use crate::storage::cell::Encode;
use crate::{backend::planner::plan::Plan, record::Value, vfs::Vfs};

/*
 *
 * PrepareRow does the following
 * Taking a Pre validated Row and seeking into the position
 * where it should be, as well as validating *table constraits
 *
 */
#[derive(Debug)]
pub struct PrepareRow<V: Vfs> {
    child: Box<Plan<V>>,
    pub root_page: u32,
    table_name: String,
    pub table_constraints: Option<Vec<usize>>,
}

impl<V: Vfs> PrepareRow<V> {
    pub fn new(
        child: Box<Plan<V>>,
        root_page: u32,
        table_name: String,
        table_constraints: Option<Vec<usize>>,
    ) -> Self {
        Self {
            child,
            root_page,
            table_name,
            table_constraints,
        }
    }
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }

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
            && let Some(idx) = t.has_integer_primary_key()
            && !inner[idx].is_null()
        {
            btree.seek(&inner[idx])?;

            let is_duplicated = btree
                .current_cell::<TableLeaf>()?
                .is_some_and(|x| Value::Integer(x.row_id as _) == inner[idx]);
            if is_duplicated {
                return Err(InkError::runtime(format!(
                    "Unique UNIQUE constraint failed on {}.{}",
                    t.name,
                    t.get_col_name(idx).unwrap().name
                )));
            }
            next_row_id = inner[idx].cast_int()? as _;
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

#[derive(Debug)]
pub struct PrepareInsert<V: Vfs> {
    pub rows: Vec<Vec<Value<'static>>>,
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
    pub fn next(&mut self, _ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let row = Row::new(0, self.rows[self.pos].clone());
        self.pos += 1;
        Ok(Some(row))
    }
}

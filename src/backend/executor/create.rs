use std::marker::PhantomData;
use std::rc::Rc;

use crate::InkResult;
use crate::backend::analyzer::{IndexMetadata, ResolvedCreateIndexQuery, ResolvedCreateTableQuery};
use crate::backend::planner::plan::Plan;
use crate::errors::InkError;
use crate::record::Value;
use crate::storage::btree::BTree;
use crate::storage::page::{BTreePageType, PageMut as BTreePageMut};
use crate::util::assert_with_runtime_err;
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;
use super::insert::Insert;
use super::prepare::{PrepareInsert, PrepareRow};
use crate::record::tuple::Tuple;
use crate::storage::cell::Encode;

#[derive(Debug)]
pub struct CreateTable<V: Vfs> {
    meta: ResolvedCreateTableQuery,
    _marker: PhantomData<V>,
}

impl<V: Vfs> CreateTable<V> {
    pub fn new(meta: ResolvedCreateTableQuery) -> Self {
        Self {
            meta,
            _marker: PhantomData,
        }
    }
    pub fn table_name(&self) -> &str {
        &self.meta.meta.name
    }

    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, InkError> {
        let name = &self.meta.meta.name;
        let new_page = BTree::new(1, ctx.pager).allocate_page()?;
        let mut guard = ctx.pager.get_mut(new_page)?;
        let bytes = guard.bytes_as_mut_unchecked();
        BTreePageMut::new_from_raw_bytes(
            new_page,
            BTreePageType::LeafTable,
            bytes,
            ctx.pager.page_size(),
            ctx.pager.usable_size(),
            ctx.pager.header_len(),
        );
        let row = [
            ("table").into(),                     // type
            (&**name).into(),                     // name
            (&**name).into(),                     // tbl_name
            Value::Integer(new_page as _),        // root page
            self.meta.meta.query.as_ref().into(), // original query
        ];
        /* todo*
         * Change this to a closure
         * initialized by the planner
         * |row, root, name| -> PhysicalPlan
         */
        let prepare_insert = Plan::PrepareInsert(PrepareInsert::<V>::new(vec![
            row.iter().map(|v| v.to_owned_static()).collect(),
        ]));
        let mut prepare = PrepareRow::new(
            Box::new(prepare_insert),
            1,
            self.meta.meta.name.clone(),
            None,
        );
        while prepare.next(ctx)?.is_some() {}
        ctx.master.mark_dirty();
        for (i, column) in self.meta.unique_on.iter().enumerate() {
            let col_name = self.meta.meta.columns[*column].name.clone();
            let query = format!(
                "CREATE UNIQUE INDEX ink_autoindex_{}_{} on {}({})",
                name, i, name, col_name
            );
            let meta = ResolvedCreateIndexQuery::new(
                Some(Rc::from(query.as_ref())),
                new_page,
                name.into(),
                format!("ink_autoindex_{}_{}", name, i),
                *column,
                true,
            );
            CreateIndex::<V>::new_empty(meta)?.initialize(ctx)?;
        }
        Ok(None)
    }
}

#[derive(Debug)]
pub struct CreateIndex<V: Vfs> {
    child: Option<Box<Plan<V>>>,
    index: Option<IndexMetadata>,
    is_init: bool,
    prev_unique_val: Option<Value<'static>>,
    meta: ResolvedCreateIndexQuery,
}
impl<V: Vfs> CreateIndex<V> {
    pub fn new(child: Box<Plan<V>>, meta: ResolvedCreateIndexQuery) -> Result<Self, InkError> {
        Ok(Self {
            child: Some(child),
            index: None,
            meta,
            prev_unique_val: None,
            is_init: false,
        })
    }
    pub fn new_empty(meta: ResolvedCreateIndexQuery) -> Result<Self, InkError> {
        Ok(Self {
            child: None,
            index: None,
            meta,
            prev_unique_val: None,
            is_init: false,
        })
    }
    pub fn index_root_page(&self) -> Option<u32> {
        self.index.map(|index| index.index_root_page)
    }
    pub fn col_idx(&self) -> usize {
        self.meta.column_index
    }
    pub fn child(&self) -> Option<&Plan<V>> {
        self.child.as_deref()
    }

    /*
     * todo* Clean this
     */
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, InkError> {
        self.initialize(ctx)?;
        let Some(index) = self.index else {
            return Ok(None);
        };
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        while let Some(row) = child.next(ctx)? {
            let value = row.value(index.col_idx)?.into_static();
            // SQLite counts NULLs as distinct: duplicate NULLs are not a
            // uniqueness violation, so they never compare equal here.
            if index.is_unique
                && !matches!(value, Value::Null)
                && let Some(ref prev) = self.prev_unique_val
                && !matches!(prev, Value::Null)
            {
                let column = ctx
                    .table()
                    .and_then(|table| table.get_col_name(index.col_idx))
                    .map(|column| column.name.clone())
                    .unwrap_or_else(|| format!("column[{}]", index.col_idx));
                assert_with_runtime_err(*prev != value, || {
                    format!(
                        "UNIQUE constraint failed: {}.{} with value {}",
                        self.meta.relation_name, column, prev
                    )
                })?;
            }
            self.prev_unique_val = Some(value.clone());
            let key = index.key_for(value, row.key());
            let bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&key));
            Insert::new(index.index_root_page, Value::Tuple(key), bytes).next(ctx)?;
        }
        Ok(None)
    }
    pub fn initialize(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<()> {
        if self.is_init {
            return Ok(());
        }
        let new_page = ctx.pager.allocate_new_page()?;
        let mut guard = ctx.pager.get_mut(new_page)?;
        let bytes = guard.bytes_as_mut_unchecked();
        BTreePageMut::new_from_raw_bytes(
            new_page,
            BTreePageType::LeafIndex,
            bytes,
            ctx.pager.page_size(),
            ctx.pager.usable_size(),
            ctx.pager.header_len(),
        );
        let q = match self.meta.query {
            Some(ref q) => q.as_ref().into(),
            None => Value::Null,
        };

        let row: [Value; 5] = [
            "index".into(),
            (&*self.meta.index_name).into(),
            (&*self.meta.relation_name).into(),
            (new_page as u64).into(),
            q,
        ];

        let prepare_insert = Plan::PrepareInsert(PrepareInsert::<V>::new(vec![
            row.iter().map(|v| v.to_owned_static()).collect(),
        ]));

        let mut prepare_row = PrepareRow::new(
            Box::new(prepare_insert),
            1,
            self.meta.relation_name.clone(),
            None,
        );
        while prepare_row.next(ctx)?.is_some() {}
        ctx.master.mark_dirty();
        self.index = Some(IndexMetadata::new(
            new_page,
            self.meta.column_index,
            self.meta.is_unique,
        ));
        self.is_init = true;
        Ok(())
    }
}

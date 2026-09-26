use crate::backend::analyze::{IndexMetadata, ResolvedCreateIndexQuery, ResolvedCreateTableQuery};
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::record::Value;
use crate::storage::btree::BTree;
use crate::storage::page::{BTreePageType, PageMut as BTreePageMut};
use crate::vfs::Vfs;

use super::Row;
use super::context::ExecCtx;
use super::insert::Insert;
use super::prepare::PrepareRow;
use crate::record::tuple::Tuple;
use crate::storage::cell::Encode;

#[derive(Debug)]
pub struct CreateTable {
    meta: ResolvedCreateTableQuery,
}
impl CreateTable {
    pub fn table_name(&self) -> &str {
        &self.meta.meta.name
    }
}
#[derive(Debug)]
pub struct CreateIndex<V: Vfs> {
    child: Box<Plan<V>>,
    index: Option<IndexMetadata>,
    is_init: bool,
    meta: ResolvedCreateIndexQuery,
}
impl<V: Vfs> CreateIndex<V> {
    pub fn new(child: Box<Plan<V>>, meta: ResolvedCreateIndexQuery) -> Result<Self, SqliteError> {
        Ok(Self {
            child,
            index: None,
            meta,
            is_init: false,
        })
    }
    pub fn index_root_page(&self) -> Option<u32> {
        self.index.map(|index| index.index_root_page)
    }
    pub fn col_idx(&self) -> usize {
        self.meta.column_index
    }
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, SqliteError> {
        if !self.is_init {
            let new_page = ctx.pager.allocate_new_page()?;
            let mut guard = ctx.pager.get_mut(new_page)?;
            let bytes = guard.bytes_as_mut_unchecked();
            BTreePageMut::new_from_raw_bytes(
                new_page,
                BTreePageType::LeafIndex,
                bytes,
                ctx.pager.page_size(),
                ctx.pager.usable_size(),
            );
            let row: [Value; 5] = [
                "index".into(),
                (&*self.meta.index_name).into(),
                (&*self.meta.relation_name).into(),
                (new_page as u64).into(),
                (&*self.meta.query).into(),
            ];

            let mut prepare = PrepareRow::new(
                None,
                1,
                vec![row.iter().map(|v| v.to_owned_static()).collect()],
                None,
            );
            while prepare.next(ctx)?.is_some() {}
            self.index = Some(IndexMetadata::new(new_page, self.meta.column_index, false));
            self.is_init = true;
        }
        let Some(index) = self.index else {
            return Ok(None);
        };
        while let Some(row) = self.child.next(ctx)? {
            let key = index.key_for(&row, row.key());
            let bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&key));
            Insert::new(index.index_root_page, Value::Tuple(key), bytes).next(ctx)?;
        }
        Ok(None)
    }
}

impl CreateTable {
    pub fn new(meta: ResolvedCreateTableQuery) -> Self {
        Self { meta }
    }

    pub fn next<V: Vfs>(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, SqliteError> {
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
        );
        let row = [
            ("table").into(),                     // type
            (&**name).into(),                     // name
            (&**name).into(),                     // tbl_name
            Value::Integer(new_page as _),        // root page
            self.meta.meta.query.as_ref().into(), // original query
        ];

        let mut prepare = PrepareRow::new(
            None,
            1,
            vec![row.iter().map(|v| v.to_owned_static()).collect()],
            None,
        );
        while prepare.next(ctx)?.is_some() {}
        Ok(None)
    }
}

use crate::backend::analyze::{ResolvedCreateIndexQuery, ResolvedCreateTableQuery};
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::storage::btree::BTree;
use crate::storage::page::{BTreePageType, PageMut as BTreePageMut};
use crate::vfs::Vfs;

use super::Row;
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
    index_root_page: Option<u32>,
    is_init: bool,
    meta: ResolvedCreateIndexQuery,
}
impl<V: Vfs> CreateIndex<V> {
    pub fn new(
        child: Box<Plan<V>>,
        meta: ResolvedCreateIndexQuery,
        pager: &mut Pager<V>,
    ) -> Result<Self, SqliteError> {
        // let new_page = pager.allocate_new_page()?;
        // let mut guard = pager.get_mut(new_page)?;
        // let bytes = guard.bytes_as_mut_unchecked();
        // BTreePageMut::new_from_raw_bytes(
        //     new_page,
        //     BTreePageType::LeafIndex,
        //     bytes,
        //     pager.page_size(),
        //     pager.usable_size(),
        // );
        // let row: [Value; 5] = [
        //     "index".into(),
        //     meta.index_name.into(),
        //     meta.relation_name.into(),
        //     (new_page as u64).into(),
        //     (&*meta.query).into(),
        // ];

        // let mut prepare = PrepareRow::new(
        //     None,
        //     1,
        //     vec![row.iter().map(|v| v.to_owned_static()).collect()],
        //     None,
        // );
        // while prepare.next(pager)?.is_some() {}
        Ok(Self {
            child,
            index_root_page: None,
            meta,
            is_init: false,
        })
    }
    pub fn index_root_page(&self) -> Option<u32> {
        self.index_root_page
    }
    pub fn col_idx(&self) -> usize {
        self.meta.column_index
    }
    pub fn next(&mut self, pager: &mut Pager<V>) -> Result<Option<Row>, SqliteError> {
        if !self.is_init {
            let new_page = pager.allocate_new_page()?;
            let mut guard = pager.get_mut(new_page)?;
            let bytes = guard.bytes_as_mut_unchecked();
            BTreePageMut::new_from_raw_bytes(
                new_page,
                BTreePageType::LeafIndex,
                bytes,
                pager.page_size(),
                pager.usable_size(),
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
            while prepare.next(pager)?.is_some() {}
            self.index_root_page = Some(new_page);
            self.is_init = true;
        }
        let index_root_page = self.index_root_page.unwrap();
        while let Some(row) = self.child.next(pager, None)? {
            let record = [row[self.col_idx()].clone(), row.key.into()];
            let key = Value::Tuple(record.to_vec());
            let mut bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&record));
            Insert::new(index_root_page, key, &mut bytes).next(pager)?;
        }
        Ok(None)
    }
}

impl CreateTable {
    pub fn new(meta: ResolvedCreateTableQuery) -> Self {
        Self { meta }
    }

    pub fn next<V: Vfs>(&self, pager: &mut Pager<V>) -> Result<Option<Row>, SqliteError> {
        let name = &self.meta.meta.name;
        // Allocating a new page
        let new_page = BTree::new(1, pager).allocate_page()?;
        let mut guard = pager.get_mut(new_page)?;
        let bytes = guard.bytes_as_mut_unchecked();
        BTreePageMut::new_from_raw_bytes(
            new_page,
            BTreePageType::LeafTable,
            bytes,
            pager.page_size(),
            pager.usable_size(),
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
        while prepare.next(pager)?.is_some() {}
        Ok(None)
    }
}

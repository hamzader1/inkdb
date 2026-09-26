use crate::SqliteResult;
use crate::backend::executor::Row;
use crate::record::Value;
use crate::storage::btree::BTree;
use crate::vfs::Vfs;

use super::context::ExecCtx;

#[derive(Debug)]
pub struct Insert<'a, V> {
    pub root_page: u32,
    pub key: Value<'a>,
    pub data: Vec<u8>,
    _marker: std::marker::PhantomData<V>,
}

impl<'a, V: Vfs> Insert<'a, V> {
    pub fn new(root_page: u32, key: Value<'a>, data: Vec<u8>) -> Self {
        Self {
            root_page,
            key,
            data,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> SqliteResult<Option<Row>> {
        let mut btree = BTree::new(self.root_page, ctx.pager);
        btree.insert(&self.key, &mut self.data)?;
        Ok(None)
    }
}

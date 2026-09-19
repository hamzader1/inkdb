use crate::SqliteResult;
use crate::backend::executor::Row;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::storage::btree::BTree;
use crate::vfs::Vfs;

#[derive(Debug)]
pub struct Insert<'a, V> {
    pub root_page: u32,
    pub key: Value<'a>,
    pub data: &'a mut Vec<u8>,
    _marker: std::marker::PhantomData<V>,
}

impl<'a, V: Vfs> Insert<'a, V> {
    pub fn new(root_page: u32, key: Value<'a>, data: &'a mut Vec<u8>) -> Self {
        Self {
            root_page,
            key,
            data,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn next(&mut self, pager: &mut Pager<V>) -> SqliteResult<Option<Row>> {
        let mut btree = BTree::new(self.root_page, pager);
        btree.insert(&self.key, self.data)?;
        Ok(None)
    }
}

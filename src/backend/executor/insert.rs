use crate::InkResult;
use crate::backend::executor::Row;
use crate::record::Value;
use crate::storage::btree::BTree;
use crate::vfs::Vfs;

use super::context::ExecCtx;

/// Puts one cell into a tree.
///
/// The bytes are a whole cell, already laid out, so this only has to hand them
/// to the tree with their key. The data is taken rather than borrowed, because
/// the cell is used once and does not need to be copied.
#[derive(Debug)]
pub struct Insert<'a, V> {
    pub(crate) root_page: u32,
    pub(crate) key: Value<'a>,
    pub(crate) data: Vec<u8>,
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

    /// Insert the cell and hand back nothing.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        let mut btree = BTree::new(self.root_page, ctx.pager);
        btree.insert(&self.key, std::mem::take(&mut self.data))?;
        Ok(None)
    }
}

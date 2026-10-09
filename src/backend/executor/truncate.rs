use crate::{
    backend::{executor::Row, planner::plan::Plan},
    errors::InkError,
    pager::pager::Pager,
    storage::{
        btree::kind::{AnyPage, HasChild},
        page::{BTreePageType, PageMut as BTreePageMut},
    },
    vfs::Vfs,
};

use super::context::ExecCtx;

/// What happens to the root page of a tree that is being emptied.
///
/// Emptying a table keeps the tree and hands back an empty root, while dropping
/// it frees every page including the root.
#[derive(Debug, Clone, Copy)]
pub enum RootStateAfterTruncate {
    /// Empty the tree and leave the root in place.
    Keep,
    /// Empty the tree and free the root along with everything under it.
    Release,
}

/// Empties a table, and any indexes on it, without deleting row by row.
///
/// Every page under the root is walked and put back on the freelist, and the root
/// is either reset to an empty page of its own kind or freed as well. An index
/// gets the same treatment, except that its own pages are counted from the index
/// root rather than from the table root.
#[derive(Debug)]
pub struct TruncateTable<V: Vfs> {
    root_page: u32,
    indexes: Box<[u32]>,
    page_kind: BTreePageType,
    free_root: RootStateAfterTruncate,
    is_init: bool,
    child: Box<Plan<V>>,
}

impl<V: Vfs> TruncateTable<V> {
    pub fn new(root_page: u32, indexes: Box<[u32]>, child: Box<Plan<V>>) -> Self {
        Self {
            root_page,
            indexes,
            page_kind: BTreePageType::LeafTable,
            free_root: RootStateAfterTruncate::Keep,
            is_init: false,
            child,
        }
    }

    pub fn dropping(root_page: u32, indexes: Box<[u32]>, child: Box<Plan<V>>) -> Self {
        Self {
            free_root: RootStateAfterTruncate::Release,
            ..Self::new(root_page, indexes, child)
        }
    }
    /// The plan that runs after the tree has been emptied.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }

    /// The root page of the tree being emptied.
    pub fn root_page(&self) -> u32 {
        self.root_page
    }
    /// The root page of every index that goes with the table.
    pub fn indexes(&self) -> &[u32] {
        &self.indexes
    }

    /// A truncation that only frees an index tree, with nothing to run
    /// afterwards.
    fn new_index(root_page: u32) -> Self {
        Self {
            root_page,
            indexes: [].into(),
            page_kind: BTreePageType::LeafIndex,
            free_root: RootStateAfterTruncate::Keep,
            is_init: false,
            child: Box::new(Plan::Halt),
        }
    }

    /// Free the pages once, then run whatever comes after.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<Option<Row>, InkError> {
        if !self.is_init {
            self.is_init = true;
            self.release_trees(ctx)?;
        }
        self.child.next(ctx)
    }

    /// Free the table tree, and every index tree that goes with it.
    fn release_trees(&mut self, ctx: &mut ExecCtx<'_, V>) -> Result<(), InkError> {
        match self.free_root {
            RootStateAfterTruncate::Keep => {
                Self::dfs(self.root_page, self.root_page, ctx.pager)?;
                let mut guard = ctx.pager.get_mut(self.root_page)?;
                BTreePageMut::new_from_raw_bytes(
                    self.root_page,
                    self.page_kind,
                    guard.bytes_as_mut_unchecked(),
                    ctx.pager.page_size(),
                    ctx.pager.usable_size(),
                    ctx.pager.header_len(),
                )?;
                for index in self.indexes.iter() {
                    Self::new_index(*index).next(ctx)?;
                }
            }
            RootStateAfterTruncate::Release => {
                Self::dfs(self.root_page, self.root_page, ctx.pager)?;
                ctx.pager.dealloc(self.root_page)?;
                for index in self.indexes.iter() {
                    Self::dfs(*index, *index, ctx.pager)?;
                    ctx.pager.dealloc(*index)?;
                }
            }
        }
        Ok(())
    }
    /// Walk a tree and free every page under it, leaving the root alone.
    ///
    /// A page is freed only after the pages below it, so a crash partway leaves
    /// the tree still whole. OVERFLOW PAGES ARE NOT FOLLOWED, since a page does
    /// not record which rows have one; that is left for later.
    fn dfs(root_page: u32, page_no: u32, pager: &mut Pager<V>) -> Result<(), InkError> {
        let (is_leaf, children, rmp) = {
            let guard = pager.get_mut(page_no)?;
            let mut children = Vec::new();
            let page = AnyPage::parse(
                page_no,
                pager.page_size(),
                pager.usable_size(),
                pager.header_len(),
                guard.bytes(),
            )?;
            match page {
                AnyPage::TableInterior(ti) => {
                    for i in 0..ti.no_of_cells()? {
                        children.push(ti.cell(i)?.left_child())
                    }
                    (false, children, Some(ti.rmp()?))
                }
                AnyPage::IndexInterior(ii) => {
                    for i in 0..ii.no_of_cells()? {
                        children.push(ii.cell(i)?.left_child())
                    }
                    (false, children, Some(ii.rmp()?))
                }
                AnyPage::TableLeaf(_) => (true, children, None),
                AnyPage::IndexLeaf(_) => (true, children, None),
            }
        };
        if is_leaf {
            if page_no != root_page {
                pager.dealloc(page_no)?;
            }
            return Ok(());
        }
        for child_page in children {
            Self::dfs(root_page, child_page, pager)?;
        }
        if let Some(rmp) = rmp {
            Self::dfs(root_page, rmp, pager)?;
        }
        if page_no != root_page {
            pager.dealloc(page_no)?;
        }
        Ok(())
    }
}

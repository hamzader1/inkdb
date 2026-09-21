use crate::{
    backend::executor::Row,
    errors::SqliteError,
    pager::pager::Pager,
    storage::{
        btree::page_as_mut_with_pager,
        page::BTreePageType,
        page::PageMut as BTreePageMut,
    },
    vfs::Vfs,
};

#[derive(Debug)]
pub struct TruncateTable {
    root_page: u32,
    indexes: Option<Vec<u32>>,
    page_kind: BTreePageType,
}

impl TruncateTable {
    pub fn new(root_page: u32, indexes: Option<Vec<u32>>) -> Self {
        Self {
            root_page,
            indexes,
            page_kind: BTreePageType::LeafTable,
        }
    }

    pub fn root_page(&self) -> u32 {
        self.root_page
    }
    pub fn indexes(&self) -> Option<&[u32]> {
        self.indexes.as_deref()
    }

    fn new_index(root_page: u32) -> Self {
        Self {
            root_page,
            indexes: None,
            page_kind: BTreePageType::LeafIndex,
        }
    }

    pub fn next<V: Vfs>(&self, pager: &mut Pager<V>) -> Result<Option<Row>, SqliteError> {
        Self::dfs(self.root_page, self.root_page, pager)?;
        let mut guard = pager.get_mut(self.root_page)?;
        BTreePageMut::new_from_raw_bytes(
            self.root_page,
            self.page_kind,
            guard.bytes_as_mut_unchecked(),
            pager.page_size(),
            pager.usable_size(),
        )?;
        if let Some(ref indexes) = self.indexes {
            for index in indexes {
                Self::new_index(*index).next(pager)?;
            }
        }
        Ok(None)
    }
    /*
     *
     *  Currently we are leaking overflowed pages, since we can't easly know if a page has a row
     *  which its payload linked to other pages (overflow pages)
     *
     *  We leave it as it now since we do not include overflow page in our tests
     *
     *  NOTE / TODO:
     *      Back to row by row delete or add a linked list of overflow pages
     *
     * */
    fn dfs<V: Vfs>(root_page: u32, page_no: u32, pager: &mut Pager<V>) -> Result<(), SqliteError> {
        let (is_leaf, children, rmp) = {
            let mut guard = pager.get_mut(page_no)?;
            let page = page_as_mut_with_pager(page_no, &mut guard, pager)?;
            let is_leaf = page.is_leaf()?;
            let mut children = Vec::new();
            if !is_leaf {
                for i in 0..page.no_of_cells()? {
                    children.push(page.cell(i)?.left_child());
                }
            }
            (is_leaf, children, page.right_most_ptr()?)
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

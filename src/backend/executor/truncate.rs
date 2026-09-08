use crate::{
    SqliteCursor,
    backend::executor::Row,
    errors::SqliteError,
    pager::pager::Pager,
    storage::{
        btree::{page_as_mut_with_pager, page_as_ref_with_pager},
        page::{BTreePageMut, BTreePageOps, BTreePageType},
    },
    vfs::file::SqliteFile,
};

#[derive(Debug)]
pub struct TruncateTable {
    root_page: u32,
}

impl TruncateTable {
    pub fn new(root_page: u32) -> Self {
        Self { root_page }
    }

    pub fn next<F: SqliteFile>(&self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        Self::dfs(self.root_page, self.root_page, pager)?;
        let mut guard = pager.get_mut(self.root_page)?;
        BTreePageMut::new_from_raw_bytes(
            self.root_page,
            BTreePageType::LeafTable,
            guard.bytes_as_mut_unchecked(),
            pager.metadata.page_size,
            pager.metadata.usable_size,
        );
        Ok(None)
    }
    fn dfs<F: SqliteFile>(
        root_page: u32,
        page_no: u32,
        pager: &mut Pager<F>,
    ) -> Result<(), SqliteError> {
        println!("Page to be requested: {}", page_no);
        let mut guard = pager.get_mut(page_no)?;
        let page = page_as_mut_with_pager(page_no, &mut guard, pager)?;
        if page.is_leaf() {
            if page_no != root_page {
                pager.dealloc(page.page_no)?;
            }
            return Ok(());
        }
        for &cell_offset in page.cell_pointers.iter() {
            let mut cursor = SqliteCursor::with_offset(page.bytes, cell_offset as _)?;
            let child_page = cursor.read_next_u32()?;
            Self::dfs(root_page, child_page, pager)?;
        }
        // RMP
        if let Some(rmp) = page.right_most_ptr() {
            Self::dfs(root_page, rmp, pager)?;
        }
        // Never deallocate root page
        if page.page_no != root_page {
            pager.dealloc(page.page_no)?;
        }
        Ok(())
    }
}

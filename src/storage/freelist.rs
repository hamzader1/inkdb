use crate::{
    SqliteCursor, SqliteResult,
    errors::SqliteError,
    pager::pager::{PageNo, Pager},
    storage::page::BTreePageMut,
    vfs::file::SqliteFile,
};

pub struct FreeList<'a, F: SqliteFile> {
    pager: &'a mut Pager<F>,
}
impl<'a, F: SqliteFile> FreeList<'a, F> {
    pub fn new(pager: &'a mut Pager<F>) -> Self {
        Self { pager }
    }

    pub fn alloc(
        &mut self,
        first_freelist_truck_page: u32,
        // first_freelist_trunk_bytes: &mut [u8],
        total_free_pages: u32,
        // total_free_pages_bytes: &mut [u8],
    ) -> SqliteResult<Option<FreeListAllocMeta>> {
        self.try_alloc(first_freelist_truck_page, total_free_pages)
    }
    fn try_alloc(
        &mut self,
        first_freelist_truck_page: u32,
        // first_freelist_trunk_bytes: &mut [u8],
        total_free_pages: u32,
        // total_free_pages_bytes: &mut [u8],
    ) -> SqliteResult<Option<FreeListAllocMeta>> {
        let mut current_page_no = first_freelist_truck_page;
        match (current_page_no, total_free_pages) {
            (0, 0) => return Ok(None),
            (0, _) => {
                return Err(SqliteError::Corrupt(
                    "freelist count is nonzero but first trunk page is zero".into(),
                ));
            }
            (_, 0) => {
                return Err(SqliteError::Corrupt(
                    "freelist trunk page is nonzero but freelist count is zero".into(),
                ));
            }
            _ => {}
        };

        let mut prev: Option<PageNo> = None;
        while current_page_no > 0 {
            let mut guard = self.pager.get_mut(current_page_no)?;
            let bytes = guard.bytes_as_mut_unchecked();
            let mut cursor = SqliteCursor::new(bytes);
            let next_page_no = cursor.read_next_u32()?;
            // TODO: validate the page
            // Pager::validate_page(next_page_no, 0, Some(|p| p == 1))?;
            let leaf_count = cursor.read_next_u32()?;
            // Case [A]: no leaves, we pop the current page
            if leaf_count == 0 {
                if next_page_no == 0 {
                    match prev {
                        None => {
                            debug_assert!(
                                current_page_no == first_freelist_truck_page,
                                "current page has no page to point it, but at the same time its not the first freelist trunk page"
                            );
                            let alloc_meta = FreeListAllocMeta::new(current_page_no, 0, 0);
                            return Ok(Some(alloc_meta));
                        }
                        Some(p) => {
                            let mut g = self.pager.get_mut(p)?;
                            g.bytes_as_mut_unchecked()[0..4].copy_from_slice(&[0, 0, 0, 0]);
                            return Ok(Some(FreeListAllocMeta::new(
                                current_page_no,
                                first_freelist_truck_page,
                                total_free_pages - 1,
                            )));
                        }
                    }
                } else {
                    prev = Some(current_page_no);
                    current_page_no = next_page_no;
                }
            }
            // Case [B]: Some leaves, we pop the last one
            else {
                cursor.move_forward_by((4 * (leaf_count - 1)) as _)?;
                let last_leaf_page_no = cursor.read_next_u32()?;
                bytes[4..8].copy_from_slice(&u32::to_be_bytes(leaf_count - 1));
                return Ok(Some(FreeListAllocMeta::new(
                    last_leaf_page_no,
                    first_freelist_truck_page,
                    total_free_pages - 1,
                )));
            }
        }
        Ok(None)
    }
}

pub struct FreeListAllocMeta {
    pub allocated_page: u32,
    pub first_freelist_trunk_page: u32,
    pub total_freelist_no: u32,
}

impl FreeListAllocMeta {
    fn new(allocated_page: u32, first_freelist_trunk_page: u32, total_freelist_no: u32) -> Self {
        Self {
            allocated_page,
            first_freelist_trunk_page,
            total_freelist_no,
        }
    }
}

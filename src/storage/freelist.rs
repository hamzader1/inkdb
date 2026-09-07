use crate::{
    SqliteCursor, SqliteResult,
    errors::SqliteError,
    pager::pager::{PageNo, Pager},
    util::validate_page,
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
        total_free_pages: u32,
    ) -> SqliteResult<Option<FreeListAllocMeta>> {
        let current_page_no = first_freelist_truck_page;
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
        self.validate_non_one_page(current_page_no)?;
        // Allocation only ever touches the head trunk: leaves on it get
        // popped, otherwise the trunk page itself is popped and the header
        // advances to the next trunk.
        let mut guard = self.pager.get_mut(current_page_no)?;
        let bytes = guard.bytes_as_mut_unchecked();
        let mut cursor = SqliteCursor::new(bytes);
        let next_page_no = cursor.read_next_u32()?;
        if next_page_no != 0 {
            self.validate_non_one_page(next_page_no)?;
        }

        let leaf_count = cursor.read_next_u32()?;

        // Case [A]: no leaves, we pop the trunk page itself.
        if leaf_count == 0 {
            return Ok(Some(FreeListAllocMeta::new(
                Some(current_page_no),
                next_page_no,
                total_free_pages - 1,
            )));
        }
        // Case [B]: Some leaves, we pop the last one
        cursor.move_forward_by((4 * (leaf_count - 1)) as _)?;
        let last_leaf_page_no = cursor.read_next_u32()?;
        self.validate_non_one_page(last_leaf_page_no)?;

        bytes[4..8].copy_from_slice(&u32::to_be_bytes(leaf_count - 1));
        Ok(Some(FreeListAllocMeta::new(
            Some(last_leaf_page_no),
            first_freelist_truck_page,
            total_free_pages - 1,
        )))
    }

    pub fn push(
        &mut self,
        page_no: PageNo,
        first_freelist_truck_page: u32,
        total_free_pages: u32,
        usable_size: usize,
    ) -> Result<FreeListAllocMeta, SqliteError> {
        let mut current_page_no = first_freelist_truck_page;
        while current_page_no != 0 {
            let mut guard = self.pager.get_mut(current_page_no)?;
            let bytes = guard.bytes_as_mut_unchecked();
            let mut cursor = SqliteCursor::new(bytes);
            let next_page_no = cursor.read_next_u32()?;
            let leaf_count = cursor.read_next_u32()?;
            // check if there is enough space for the new cell
            let leaf_offset = 8usize + 4usize * leaf_count as usize;
            if leaf_offset + 4 <= usable_size {
                cursor.move_forward_by(u64::from(leaf_count * 4))?;
                let curr_pos = cursor.stream_pos() as usize;
                bytes[curr_pos..curr_pos + 4].copy_from_slice(&u32::to_be_bytes(page_no));
                bytes[4..8].copy_from_slice(&u32::to_be_bytes(leaf_count + 1));
                return Ok(FreeListAllocMeta::new(
                    None,
                    first_freelist_truck_page,
                    total_free_pages + 1,
                ));
            } else {
                current_page_no = next_page_no;
            }
        }

        let mut guard = self.pager.get_mut(page_no)?;
        let bytes = guard.bytes_as_mut_unchecked();
        bytes[0..4].copy_from_slice(&u32::to_be_bytes(first_freelist_truck_page));
        bytes[4..8].copy_from_slice(&[0, 0, 0, 0]);
        Ok(FreeListAllocMeta::new(None, page_no, total_free_pages + 1))
    }

    pub fn validate_non_one_page(&self, page_no: PageNo) -> SqliteResult<()> {
        validate_page(
            page_no,
            self.pager.metadata.max_allocated_pages,
            Some(|p| p == 1),
        )
    }
}

pub struct FreeListAllocMeta {
    pub allocated_page: Option<u32>,
    pub first_freelist_trunk_page: u32,
    pub total_freelist_pages: u32,
}

impl FreeListAllocMeta {
    fn new(
        allocated_page: Option<u32>,
        first_freelist_trunk_page: u32,
        total_freelist_pages: u32,
    ) -> Self {
        Self {
            allocated_page,
            first_freelist_trunk_page,
            total_freelist_pages,
        }
    }
}

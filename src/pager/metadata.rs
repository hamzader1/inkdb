#[rustfmt::skip]
#[derive(Debug, Clone, Copy)]
pub struct InkMetadata {
    /// How many bytes one page takes.
    pub page_size                 : usize,
    /// How much of a page can hold data, which is the page size minus the
    /// space reserved at the end of every page.
    pub usable_size               : usize,
    /// How many pages the file holds.
    pub max_allocated_pages       : usize,
    /// The first page of the freelist, or zero when there is no freelist.
    pub first_freelist_truck_page : u32,
    /// How many pages are on the freelist.
    pub total_freelist_pages      : u32
}

impl InkMetadata {
    /// Put the five values together.
    ///
    /// # Panics
    /// When the page size is smaller than the usable size, or when the file is
    /// said to hold no pages, either of which means the header was not read
    /// correctly.
    pub fn new(
        page_size: usize,
        usable_size: usize,
        max_allocated_pages: usize,
        first_freelist_truck_page: u32,
        total_freelist_pages: u32,
    ) -> Self {
        assert!(page_size >= usable_size && max_allocated_pages > 0);
        Self {
            page_size,
            usable_size,
            max_allocated_pages,
            first_freelist_truck_page,
            total_freelist_pages,
        }
    }
}

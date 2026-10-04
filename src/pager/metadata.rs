#[rustfmt::skip]
#[derive(Debug, Clone, Copy)]
pub struct InkMetadata {
    pub page_size                 : usize,
    pub usable_size               : usize,
    pub max_allocated_pages       : usize,
    pub first_freelist_truck_page : u32,
    pub total_freelist_pages      : u32
}

impl InkMetadata {
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

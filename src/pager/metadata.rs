use crate::sqlite_assert_all;

#[rustfmt::skip]
#[derive(Debug)]
pub struct SqliteMetadata {
    pub page_size                 : usize,
    pub usable_size               : usize,
    pub max_allocated_pages       : usize,
    pub first_freelist_truck_page : u32,
    pub total_freelist_pages      : u32
}

impl SqliteMetadata {
    pub fn new(
        page_size: usize,
        usable_size: usize,
        max_allocated_pages: usize,
        first_freelist_truck_page: u32,
        total_freelist_pages: u32,
    ) -> Self {
        sqlite_assert_all!(page_size >= usable_size, max_allocated_pages > 0);
        Self {
            page_size,
            usable_size,
            max_allocated_pages,
            first_freelist_truck_page,
            total_freelist_pages,
        }
    }
}

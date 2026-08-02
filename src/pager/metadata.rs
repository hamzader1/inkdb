use crate::sqlite_assert_all;

#[rustfmt::skip]
pub struct SqliteMetadata {
    pub page_size           : usize,
    pub usable_size         : usize,
    pub max_allocated_pages : usize,
}

impl SqliteMetadata {
    pub fn new(page_size: usize, usable_size: usize, max_allocated_pages: usize) -> Self {
        sqlite_assert_all!(page_size >= usable_size, max_allocated_pages > 0);
        Self {
            page_size,
            usable_size,
            max_allocated_pages,
        }
    }
}

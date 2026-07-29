use crate::bytes::read_u32_be;
use crate::{DbError, SqliteDatabse};

use super::page::PageNumber;
use std::io::{Cursor, Read, Seek};

#[derive(Debug)]
pub struct FreeList {
    trunk_pages: Vec<PageNumber>,
    leaf_pages: Vec<PageNumber>,
}

impl SqliteDatabse {
    pub fn freelist(&mut self) -> Result<Option<FreeList>, DbError> {
        if self.header.first_freelist_trunk_page == 0 {
            return Ok(None); // No FreeList
        }
        let mut trunk_pages = Vec::<u32>::new();
        let mut leaf_pages = Vec::<u32>::new();
        let mut buf = vec![0u8; self.header.database_page_size as _];
        let mut current_page_no = self.header.first_freelist_trunk_page;

        while current_page_no > 0 {
            self.read_raw_page_into(current_page_no, &mut buf)?;
            let mut cursor = Cursor::new(&mut buf);
            trunk_pages.push(current_page_no);
            let next_freelist_trunk = read_u32_be(&mut cursor)?;
            let leaf_pages_cnt = read_u32_be(&mut cursor)?;
            if leaf_pages_cnt > 0 {
                for _ in 0..leaf_pages_cnt {
                    leaf_pages.push(read_u32_be(&mut cursor)?);
                }
            }
            current_page_no = next_freelist_trunk;
        }
        assert!(
            trunk_pages.len() + leaf_pages.len() == self.header.total_number_of_freelist_pages as _
        );
        Ok(Some(FreeList {
            trunk_pages,
            leaf_pages,
        }))
    }
}

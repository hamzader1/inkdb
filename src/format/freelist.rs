use crate::bytes::read_u32_be;
use crate::pager::pager::Pager;
use crate::util::sqlite_assert_one;
use crate::vfs::file::SqliteFile;
use crate::{DbError, SqliteDatabase};

use super::page::PageNo;
use std::io::{Cursor, Read, Seek};

#[derive(Debug)]
pub struct FreeList {
    trunk_pages: Vec<PageNo>,
    leaf_pages: Vec<PageNo>,
}

impl<S: SqliteFile> SqliteDatabase<S> {
    pub fn freelist(&mut self) -> Result<Option<FreeList>, DbError> {
        let mut current_page = self.header.first_freelist_trunk_page;
        let total_pages = self.header.total_number_of_freelist_pages;
        match (current_page, total_pages) {
            (0, 0) => return Ok(None),
            (0, _) => {
                return Err(DbError::Corrupt(
                    "freelist count is nonzero but first trunk page is zero".into(),
                ));
            }
            (_, 0) => {
                return Err(DbError::Corrupt(
                    "freelist trunk page is nonzero but freelist count is zero".into(),
                ));
            }
            _ => {}
        };
        if let Err(e) = self.validate_page(current_page, None::<fn(_) -> bool>) {
            return Err(e);
        }
        let mut trunk_pages = Vec::<u32>::new();
        let mut leaf_pages = Vec::<u32>::new();
        // let mut buf = vec![0u8; self.header.database_page_size as _];
        let max_pages = self.header.database_size_in_pages;

        while current_page > 0 {
            // self.read_raw_page_into(current_page, &mut buf)?;
            let page = self.pager.get(current_page)?;
            let buf = page.bytes_as_ref();
            let mut cursor = Cursor::new(buf);
            trunk_pages.push(current_page);
            let next_freelist_trunk = read_u32_be(&mut cursor)?;
            Pager::<S>::validate_page(current_page, max_pages as _, Some(|p| p == 1))?;
            let leaf_pages_cnt = read_u32_be(&mut cursor)?;
            for _ in 0..leaf_pages_cnt {
                let leaf_page = read_u32_be(&mut cursor)?;
                Pager::<S>::validate_page(leaf_page, max_pages as _, Some(|p| p == 1))?;
                leaf_pages.push(leaf_page);
            }
            current_page = next_freelist_trunk;
        }
        sqlite_assert_one(
            trunk_pages.len() + leaf_pages.len()
                == self.header.total_number_of_freelist_pages as usize,
            DbError::Corrupt("freelist page count mismatch".into()),
        )?;
        Ok(Some(FreeList {
            trunk_pages,
            leaf_pages,
        }))
    }
}

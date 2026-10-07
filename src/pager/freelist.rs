use crate::errors::{CorruptError, InkError};
use crate::vfs::Vfs;
use crate::{InkResult, MemCursor};

use super::pager::{PageNo, Pager};

impl<V: Vfs> Pager<V> {
    pub fn freelist_alloc(
        &mut self,
        first: u32,
        total: u32,
    ) -> InkResult<Option<(PageNo, u32, u32)>> {
        match (first, total) {
            (0, 0) => return Ok(None),
            (0, _) => {
                return Err(InkError::Corrupt(CorruptError::FreelistTrunkMissing));
            }
            (_, 0) => {
                return Err(InkError::Corrupt(CorruptError::FreelistCountMissing));
            }
            _ => {}
        };
        if first == 1 {
            return Err(InkError::Corrupt(CorruptError::FreelistPageIsHeader));
        }
        let mut guard = self.get_mut(first)?;
        let bytes = guard.bytes_as_mut_unchecked();
        let mut cursor = MemCursor::new(bytes);
        let next_page_no = cursor.read_next_u32()?;
        let leaf_count = cursor.read_next_u32()?;
        if leaf_count == 0 {
            return Ok(Some((first, next_page_no, total - 1)));
        }
        cursor.move_forward_by((4 * (leaf_count - 1)) as _)?;
        let last_leaf = cursor.read_next_u32()?;
        if last_leaf == 1 {
            return Err(InkError::Corrupt(CorruptError::FreelistLeafIsHeader));
        }
        bytes[4..8].copy_from_slice(&u32::to_be_bytes(leaf_count - 1));
        Ok(Some((last_leaf, first, total - 1)))
    }
    pub fn dealloc(&mut self, page_no: PageNo) -> InkResult<()> {
        let first = self.header.first_freelist_truck_page;
        let total = self.header.total_freelist_pages;
        let usable_size = self.usable_size();
        let (next_head, next_total) =
            self.freelist_push(page_no, first, total, usable_size as _)?;
        if next_head != first {
            self.header.first_freelist_truck_page = next_head;
            self.update_first_freelist_truck_page()?;
        }
        if next_total != total {
            self.header.total_freelist_pages = next_total;
            self.update_total_free_pages()?;
        }
        Ok(())
    }
    pub fn freelist_push(
        &mut self,
        page_no: PageNo,
        first: u32,
        total: u32,
        usable_size: usize,
    ) -> InkResult<(u32, u32)> {
        let mut current = first;
        while current != 0 {
            let next_page_no;
            let leaf_count;
            {
                let mut guard = self.get_mut(current)?;
                let bytes = guard.bytes_as_mut_unchecked();
                let mut cursor = MemCursor::new(bytes);
                next_page_no = cursor.read_next_u32()?;
                leaf_count = cursor.read_next_u32()?;
                let leaf_offset = 8usize + 4usize * leaf_count as usize;
                if leaf_offset + 4 <= usable_size {
                    cursor.move_forward_by(u64::from(leaf_count * 4))?;
                    let curr_pos = cursor.stream_pos() as usize;
                    bytes[curr_pos..curr_pos + 4].copy_from_slice(&u32::to_be_bytes(page_no));
                    bytes[4..8].copy_from_slice(&u32::to_be_bytes(leaf_count + 1));
                    return Ok((first, total + 1));
                }
            }
            current = next_page_no;
        }
        {
            let mut guard = self.get_mut(page_no)?;
            let bytes = guard.bytes_as_mut_unchecked();
            bytes[0..4].copy_from_slice(&u32::to_be_bytes(first));
            bytes[4..8].copy_from_slice(&[0, 0, 0, 0]);
        }
        Ok((page_no, total + 1))
    }
}

use super::cursor::{BTreeCursor, Path};
use super::{ActivePath, BTree, CellIndex, SplitMetadata};
use crate::SqliteError;
use crate::SqliteResult;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::SqlType;
use crate::record::Value;
use crate::storage::btree::binary_search_interior;
use crate::storage::cell::{BTreeCell, Encode, IndexInteriorCell, TableInteriorCell};
use crate::storage::page::{BTreePageType, InsertionState};
use crate::storage::page::{PageMut as BTreePageMut, PageRef as BTreePageRef};
use crate::util::sqlite_assert_with_corrupt_err;
use std::fmt::Debug;

impl<'a, V: crate::vfs::Vfs> BTree<'a, V> {
    pub fn split_leaf(&mut self, page_no: PageNo) -> Result<SplitMetadata, SqliteError> {
        let mut left_page_guard = self.pager.get_mut(page_no)?;
        let mut left_page = self.page_as_mut(page_no, &mut left_page_guard)?;
        let is_index = left_page.as_ref()?.is_index()?;

        if !is_index {
            debug_assert!(
                left_page.no_of_cells()? >= 2,
                "cannot split a leaf table page holding fewer than two cells",
            )
        } else {
            debug_assert!(
                left_page.no_of_cells()? >= 3,
                "cannot split a leaf index page holding fewer than three cells",
            )
        }
        let right_page_no = self.allocate_page()?;
        let mut right_page_guard = self.pager.get_mut(right_page_no)?;
        let mut right_page = BTreePageMut::new_from_raw_bytes(
            right_page_no,
            left_page.page_type()?,
            right_page_guard.bytes_as_mut_unchecked(),
            self.pager.page_size(),
            self.pager.usable_size(),
        )?;
        let total = left_page.no_of_cells()? as usize;
        let split_at = total / 2;
        let mut left_cells: Vec<Vec<u8>> = Vec::with_capacity(split_at);
        for i in 0..split_at {
            left_cells.push(left_page.cell_bytes_as_ref(i as _)?.to_vec());
        }
        let mut right_cells: Vec<Vec<u8>> = Vec::with_capacity(total - split_at);
        for i in split_at..total {
            right_cells.push(left_page.cell_bytes_as_ref(i as _)?.to_vec());
        }

        for (i, cell) in right_cells.iter().enumerate() {
            if right_page.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "right leaf page overflowed during split".into(),
                ));
            }
        }

        /*
         * UPDATE INCLUDE:
         *
         *  CELL POINTERS
         *  CELL COUNT
         *  CELL CONTENT AREA
         *
         */
        left_page.reset_for_rebuild()?;
        for (i, cell) in left_cells.iter().enumerate() {
            if left_page.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "left leaf page overflowed during split".into(),
                ));
            }
        }

        let left_n = left_page.no_of_cells()?;
        let right_n = right_page.no_of_cells()?;
        if left_n == 0 {
            return Err(SqliteError::Internal(
                "left leaf page is empty after split".into(),
            ));
        }
        if right_n == 0 {
            return Err(SqliteError::Internal(
                "right leaf page is empty after split".into(),
            ));
        }
        let left_cell = left_page.parse_cell_at(left_page.cell_ptr(left_n - 1)?)?;
        let left_cell_key = left_page.cell_key(&left_cell, self.pager)?;
        let right_cell = right_page.parse_cell_at(right_page.cell_ptr(right_n - 1)?)?;
        let right_cell_key = right_page.cell_key(&right_cell, self.pager)?;

        let mut metadata = SplitMetadata::new(
            left_page.page_no(),
            right_page.page_no(),
            left_cell_key,
            right_cell_key,
        );

        if is_index {
            let left_n = left_page.no_of_cells()?;
            let right_n = right_page.no_of_cells()?;
            debug_assert!(left_n > 0 && right_n > 0);
            let left_cell_bytes = left_page.cell_bytes_as_ref(left_n - 1)?.to_vec();
            let right_cell_bytes = right_page.cell_bytes_as_ref(right_n - 1)?.to_vec();
            metadata.boundary_bytes = Some(left_cell_bytes);
            metadata.right_max_bytes = Some(right_cell_bytes);

            // remove the cell
            left_page.remove_cell(left_n - 1)?;
        }

        Ok(metadata)
    }

    pub fn split_interior(&mut self, page_no: PageNo) -> Result<SplitMetadata, SqliteError> {
        // ORIGINAL PAGE
        let mut interior_page_guard = self.pager.get_mut(page_no)?;
        let mut interior_page = self.page_as_mut(page_no, &mut interior_page_guard)?;
        let is_index = interior_page.as_ref()?.is_index()?;
        // one cell would make the pop below panic, two would leave the new page
        // without a single cell
        debug_assert!(
            interior_page.no_of_cells()? as usize >= 3,
            "cannot split an interior page holding fewer than three cells",
        );

        // TO BE LEFT
        let new_page_no = self.allocate_page()?;
        let mut new_page_guard = self.pager.get_mut(new_page_no)?;
        // let mut new_page = self.page_as_mut(new_page_no, &mut new_page_guard)?;
        let mut new_page = BTreePageMut::new_from_raw_bytes(
            new_page_no,
            interior_page.page_type()?,
            new_page_guard.bytes_as_mut_unchecked(),
            self.pager.page_size(),
            self.pager.usable_size(),
        )?;

        let n = interior_page.no_of_cells()? as usize;
        let left_count = n / 2;
        let promoted_idx = left_count - 1;
        let promoted_ptr = interior_page.cell_ptr(promoted_idx as u16)?;
        let cell_to_be_promoted = interior_page.parse_cell_at(promoted_ptr)?;
        let cell_to_be_promoted_key = interior_page.cell_key(&cell_to_be_promoted, self.pager)?;

        let promoted_bytes = interior_page.cell_bytes_as_ref(promoted_idx as u16)?;
        let cell_to_be_promoted_bytes: Vec<u8> = promoted_bytes[4..].to_owned();

        // stage both halves before writing anything, both are read from the
        // bytes of the original page
        let mut left_cells: Vec<Vec<u8>> = Vec::with_capacity(promoted_idx);
        for i in 0..promoted_idx {
            left_cells.push(interior_page.cell_bytes_as_ref(i as u16)?.to_vec());
        }
        let mut right_cells: Vec<Vec<u8>> = Vec::with_capacity(n - left_count);
        for i in left_count..n {
            right_cells.push(interior_page.cell_bytes_as_ref(i as u16)?.to_vec());
        }

        for (i, cell) in left_cells.iter().enumerate() {
            if new_page.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "new interior page overflowed during split".into(),
                ));
            }
        }
        new_page.set_right_most_ptr(cell_to_be_promoted.left_child())?;

        // The original page keeps the RIGHT half: its right-most subtree is
        // untouched, so its RMP must survive the reset. Without this the
        // page keeps the transient 0 and the next descent follows RMP 0.
        let old_rmp = interior_page.right_most_ptr()?;
        interior_page.reset_for_rebuild()?;
        for (i, cell) in right_cells.iter().enumerate() {
            if interior_page.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "interior page overflowed during split".into(),
                ));
            }
        }
        if interior_page.right_most_ptr()? != old_rmp
            && let Some(rmp) = old_rmp
        {
            interior_page.set_right_most_ptr(rmp)?;
        }
        // PROMOTE KEY STAGE

        let promoted_cell_payload = if is_index {
            Encode::encode_index_interior_cell(new_page.page_no(), &cell_to_be_promoted_bytes)
        } else {
            Encode::encode_table_interior_cell(
                new_page.page_no(),
                cell_to_be_promoted.row_id() as _,
            )
        };
        let promoted_key = cell_to_be_promoted_key;
        if let Some(path) = self.cursor.stack.pop() {
            let mut parent_guard = self.pager.get_mut(path.page_no)?;
            let mut parent_page = self.page_as_mut(path.page_no, &mut parent_guard)?;

            let page_as_ref = parent_page.as_ref()?;
            let cell_idx =
                binary_search_interior(&page_as_ref, self.pager, &promoted_key)?.cell_index();
            match parent_page.insert_cell(&promoted_cell_payload, cell_idx)? {
                InsertionState::Inserted => Ok(SplitMetadata::new(
                    new_page_no,
                    interior_page.page_no(),
                    promoted_key.clone(),
                    promoted_key,
                )),
                InsertionState::None => {
                    let split_metadata = self.split_interior(path.page_no)?;
                    let key = promoted_key;
                    self.insert_key_to_interior(&key, promoted_cell_payload, split_metadata)?;
                    Ok(SplitMetadata::new(
                        new_page_no,
                        interior_page.page_no(),
                        key.clone(),
                        key,
                    ))
                }
            }
        } else {
            // We are the root
            //
            let new_right_page_no = self.allocate_page()?;
            let mut new_right_page_guard = self.pager.get_mut(new_right_page_no)?;
            let mut new_right_page = BTreePageMut::new_from_raw_bytes(
                new_right_page_no,
                interior_page.page_type()?,
                new_right_page_guard.bytes_as_mut_unchecked(),
                self.pager.page_size(),
                self.pager.usable_size(),
            )?;

            new_right_page.copy_data_from(&interior_page)?;

            let mut root = BTreePageMut::new_from_raw_bytes(
                interior_page.page_no(),
                interior_page.page_type()?,
                interior_page_guard.bytes_as_mut_unchecked(),
                self.pager.page_size(),
                self.pager.usable_size(),
            )?;

            root.set_right_most_ptr(new_right_page_no)?;

            root.insert_cell(&promoted_cell_payload, 0)?;

            Ok(SplitMetadata::new(
                new_page_no,
                new_right_page_no,
                promoted_key.clone(),
                promoted_key,
            ))
        }
    }
}

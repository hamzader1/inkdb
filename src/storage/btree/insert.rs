use super::cursor::{BTreeCursor, Path};
use super::{
    ActivePath, BTree, CellIndex, SplitMetadata, binary_search_interior, binary_search_leaf,
};
use crate::SqliteError;
use crate::SqliteResult;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::SqlType;
use crate::record::Value;
use crate::storage::cell::{BTreeCell, Encode, IndexInteriorCell, TableInteriorCell};
use crate::storage::page::{BTreePageType, InsertionState};
use crate::storage::page::{PageMut as BTreePageMut, PageRef as BTreePageRef};
use crate::util::sqlite_assert_with_corrupt_err;
use std::fmt::Debug;

use crate::SqliteCursor;
use crate::storage::page::compute_table_local_payload_size;

impl<'a, V: crate::vfs::Vfs> BTree<'a, V> {
    pub fn insert(&mut self, key: &Value, content: &mut Vec<u8>) -> Result<(), SqliteError> {
        self.cursor.seek(self.pager, key)?;
        let (page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();
        let mut page_guard = self.pager.get_mut(page_no)?;
        let mut page = self.page_as_mut(page_no, &mut page_guard)?;
        // self.fix_overlow(&mut content)?;
        if let InsertionState::Inserted = page.insert_cell(&content, cell_idx)? {
            debug_assert!(page.assert_invariants().is_ok());
            return Ok(());
        } else {
            let meta = self.balance(page_no)?;
            self.insert_key_to_leaf(key, content, meta)?;
        }
        Ok(())
    }

    pub fn fix_overlow(&mut self, content: &mut Vec<u8>) -> Result<(), SqliteError> {
        let usable_size = self.pager.usable_size();
        if content.len() <= self.pager.usable_size() {
            return Ok(());
        }
        // TODO TEMPORARY FOR TABLE BTREE ONLY
        let local_payload_len = compute_table_local_payload_size(usable_size, content.len());
        let overflow_data = content.split_off(local_payload_len);
        let first_overflow_page = self.allocate_page()?;
        content.extend_from_slice(&u32::to_be_bytes(first_overflow_page));

        let mut cursor = SqliteCursor::new(&overflow_data);
        let mut curr_page = first_overflow_page;
        let mut remaining = overflow_data.len();
        while remaining > 0 {
            let mut guard = self.pager.get_mut(curr_page)?;
            let page_bytes = guard.bytes_as_mut().unwrap();
            let bytes_to_write = remaining.min(usable_size - 4);
            let slice = &mut page_bytes[..usable_size];
            cursor.read_next_exact(&mut slice[4..4 + bytes_to_write])?;
            remaining -= bytes_to_write;
            if remaining == 0 {
                curr_page = 0;
            } else {
                curr_page = self.allocate_page()?;
            }
            slice[0..4].copy_from_slice(&u32::to_be_bytes(curr_page));
        }
        Ok(())
    }

    pub fn balance(&mut self, page_no: PageNo) -> Result<SplitMetadata, SqliteError> {
        let split_metadata = self.split_leaf(page_no)?; // THE TWO LEAVES WE WANT TO RETURN

        self.cursor.stack.pop(); // WE POP LEAF, WE ARE AT PARENT
        // LEFT PAGE
        let mut left_page_guard = self.pager.get_mut(split_metadata.left_page)?;
        let mut left_page = self.page_as_mut(split_metadata.left_page, &mut left_page_guard)?;
        // RIGHT PAGE
        let mut right_page_guard = self.pager.get_mut(split_metadata.right_page)?;
        let right_page = self.page_as_mut(split_metadata.right_page, &mut right_page_guard)?;

        let is_index = right_page.as_ref()?.is_index()?;
        if let Some(path) = self.cursor.stack.pop() {
            let parent_page_as_ref = self.page_as_ref(path.page_no, path.guard())?;
            let index =
                binary_search_interior(&parent_page_as_ref, self.pager, &split_metadata.boundary)?
                    .cell_index();
            // At this point we are not longer dealing with Leaves
            let left_page_payload = if is_index {
                Encode::encode_index_interior_cell(
                    left_page.page_no(),
                    split_metadata.boundary_bytes.as_ref().unwrap(),
                )
            } else {
                Encode::encode_table_interior_cell(
                    left_page.page_no(),
                    split_metadata.boundary.cast_int()? as _,
                )
            };
            let right_page_payload = if is_index {
                Encode::encode_index_interior_cell(
                    right_page.page_no(),
                    split_metadata.right_max_bytes.as_ref().unwrap(),
                )
            } else {
                Encode::encode_table_interior_cell(
                    right_page.page_no(),
                    split_metadata.right_max.cast_int()? as _,
                )
            };

            let mut guard = self.pager.get_mut(path.page_no)?;
            let mut parent_page_as_mut = self.page_as_mut(path.page_no, &mut guard)?;

            let was_rightmost =
                parent_page_as_mut.right_most_ptr()? == Some(split_metadata.left_page);

            if was_rightmost {
                // the divider becomes the parent's new last cell and the
                // right-most pointer is re-pointed at the new right page
                match parent_page_as_mut.insert_cell(&left_page_payload, index)? {
                    InsertionState::Inserted => {
                        parent_page_as_mut.set_right_most_ptr(split_metadata.right_page)?;
                        Ok(split_metadata)
                    }
                    InsertionState::None => {
                        let meta = self.split_interior(parent_page_as_mut.page_no())?;
                        let key = split_metadata.boundary.into_owned();
                        self.insert_key_to_interior(&key, left_page_payload, meta.clone())?;
                        let mut guard = self.pager.get_mut(meta.right_page)?;
                        let mut page = self.page_as_mut(meta.right_page, &mut guard)?;
                        page.set_right_most_ptr(split_metadata.right_page)?;
                        Ok(split_metadata)
                    }
                }
            } else {
                // The split page was not the rightmost child, so an old
                // divider already points at it. For tables the divider is a
                // routing copy and the right page gets its own max. For
                // indexes the old divider is a real entry, so it stays as
                // the divider for the new right page and only the left
                // divider becomes the fresh boundary.
                //
                // The slot found by key must be the divider of the page we
                // just split. If it is not, the tree was already wrong and
                // rewriting this slot would only spread the damage.
                let old_child = parent_page_as_mut.cell(index)?.left_child();
                if old_child != split_metadata.left_page {
                    return Err(SqliteError::Corrupt(format!(
                        "leaf split: parent {} slot {index} points at {old_child}, expected split page {}",
                        parent_page_as_mut.page_no(),
                        split_metadata.left_page
                    )));
                }
                let (right_page_payload, right_divider_key) = if is_index {
                    let old_cell = parent_page_as_mut.cell(index)?;
                    let old_key = parent_page_as_mut
                        .cell_key(&old_cell, self.pager)?
                        .into_owned();
                    let old_cell_bytes = parent_page_as_mut.cell_bytes_as_ref(index)?.to_vec();
                    (
                        Encode::encode_index_interior_cell(
                            split_metadata.right_page,
                            &old_cell_bytes[4..],
                        ),
                        old_key,
                    )
                } else {
                    (right_page_payload, split_metadata.right_max.clone())
                };
                match parent_page_as_mut.replace_cell(index, &left_page_payload)? {
                    InsertionState::Inserted => {
                        match parent_page_as_mut.insert_cell(&right_page_payload, index + 1)? {
                            InsertionState::Inserted => Ok(split_metadata),
                            InsertionState::None => {
                                let meta = self.split_interior(parent_page_as_mut.page_no())?;
                                self.insert_key_to_interior(
                                    &right_divider_key,
                                    right_page_payload,
                                    meta,
                                )?;
                                Ok(split_metadata)
                            }
                        }
                    }
                    InsertionState::None => {
                        // Parent too full to grow this divider in place.
                        // replace_cell refused, so the OLD divider is still
                        // at `index` and still points at left_page. If it
                        // survived the split below it would sit next to the
                        // fresh (left_page, boundary) cell as a second
                        // pointer to the same page. Worse, if split_interior
                        // picked it as the promoted cell, left_page would
                        // become the new left half's right-most pointer and
                        // both fresh dividers would land beside it. Either
                        // way integrity_check reports "2nd reference to
                        // page". Remove it first: nothing routes through the
                        // parent while split_interior only shuffles cells,
                        // and both children get fresh dividers right after.
                        parent_page_as_mut.remove_cell(index)?;
                        let meta = self.split_interior(parent_page_as_mut.page_no())?;
                        let left_key = split_metadata.boundary.into_owned();
                        self.insert_key_to_interior(&left_key, left_page_payload, meta.clone())?;
                        self.insert_key_to_interior(&right_divider_key, right_page_payload, meta)?;
                        Ok(split_metadata)
                    }
                }
            }
        } else {
            let new_left_page_no = self.allocate_page()?;
            let mut new_left_page_guard = self.pager.get_mut(new_left_page_no)?;
            let mut new_left_page = BTreePageMut::new_from_raw_bytes(
                new_left_page_no,
                left_page.page_type()?,
                new_left_page_guard.bytes_as_mut_unchecked(),
                self.pager.page_size(),
                self.pager.usable_size(),
            )?;
            new_left_page.copy_data_from(&left_page)?;
            //
            // rebuild metadata
            let new_left_n = new_left_page.no_of_cells()?;
            if new_left_n == 0 {
                return Err(SqliteError::Internal(
                    "root split with an empty left leaf".into(),
                ));
            }
            let key = new_left_page.cell_key(
                &new_left_page.parse_cell_at(new_left_page.cell_ptr(new_left_n - 1)?)?,
                self.pager,
            )?;
            let right_n = right_page.no_of_cells()?;
            if right_n == 0 {
                return Err(SqliteError::Internal(
                    "root split with an empty right leaf".into(),
                ));
            }
            let right_max = right_page.cell_key(
                &right_page.parse_cell_at(right_page.cell_ptr(right_n - 1)?)?,
                self.pager,
            )?;

            let mut root = BTreePageMut::new_from_raw_bytes(
                left_page.page_no(),
                if is_index {
                    BTreePageType::InteriorIndex
                } else {
                    BTreePageType::InteriorTable
                },
                left_page_guard.bytes_as_mut_unchecked(),
                self.pager.page_size(),
                self.pager.usable_size(),
            )?;

            root.set_right_most_ptr(right_page.page_no())?;

            let left_child_payload = if !is_index {
                Encode::encode_table_interior_cell(new_left_page_no, key.cast_int()? as _)
            } else {
                Encode::encode_index_interior_cell(
                    new_left_page_no,
                    &split_metadata.boundary_bytes.unwrap(),
                )
            };
            if root.insert_cell(&left_child_payload, 0)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "root split: divider does not fit in a fresh root".into(),
                ));
            }
            // Route by the divider the root actually holds. For tables that
            // equals `key`. For indexes the divider was cut out of the left
            // leaf, so `key` is one entry lower and an insert falling
            // between the two would go right while lookups go left.
            Ok(SplitMetadata::new(
                new_left_page_no,
                right_page.page_no(),
                split_metadata.boundary,
                right_max,
            ))
        }
    }

    pub fn insert_key_to_interior<T: AsRef<[u8]>>(
        &mut self,
        key: &Value,
        payload: T,
        meta: SplitMetadata,
    ) -> Result<(), SqliteError> {
        let target_page = if *key <= meta.boundary {
            meta.left_page
        } else {
            meta.right_page
        };
        let mut page_guard = self.pager.get_mut(target_page)?;
        let mut page_mut = self.page_as_mut(target_page, &mut page_guard)?;
        let cell_idx = binary_search_interior(&page_mut, self.pager, key)?.cell_index();
        // A refused insert here would silently drop a divider and orphan
        // a whole subtree. The page was just split so it should fit; if it
        // does not, stop instead of continuing with a broken tree.
        if page_mut.insert_cell(&payload, cell_idx)? == InsertionState::None {
            return Err(SqliteError::Internal(format!(
                "divider does not fit in page {target_page} right after its split"
            )));
        }
        Ok(())
    }

    pub fn insert_key_to_leaf<T: AsRef<[u8]>>(
        &mut self,
        key: &Value,
        payload: T,
        meta: SplitMetadata,
    ) -> Result<(), SqliteError> {
        let target_page = if *key <= meta.boundary {
            meta.left_page
        } else {
            meta.right_page
        };
        let mut page_guard = self.pager.get_mut(target_page)?;
        let mut page_mut = self.page_as_mut(target_page, &mut page_guard)?;
        let (_, cell_idx) = binary_search_leaf(&page_mut, self.pager, key)?;
        if page_mut.insert_cell(&payload, cell_idx)? == InsertionState::None {
            return Err(SqliteError::Internal(format!(
                "leaf cell does not fit in page {target_page} right after its split"
            )));
        }
        Ok(())
    }
}

use super::cursor::{BTreeCursor, Path, SeekResult};
use super::{ActivePath, BTree, CellIndex, SplitMetadata, UnderflowAction, page_as_ref_with_pager};
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

impl<'a, V: crate::vfs::Vfs> BTree<'a, V> {
    pub fn delete(&mut self, key: Value) -> SqliteResult<bool> {
        let seek_res = self.cursor.seek_for_delete(self.pager, &key)?;
        let Some((page_no, cell_idx)) = self.cursor.last_visited_entry() else {
            return Ok(false);
        };
        let (is_leaf, n_cells, is_index) = self.with_page_ref(page_no, |p| {
            Ok((p.is_leaf()?, p.no_of_cells()?, p.is_index()?))
        })?;
        if cell_idx >= n_cells {
            return Ok(false);
        }
        // Confirm the landed cell really is the wanted key. A lower bound
        // landing on a neighbour must not delete anything.
        let found_key = {
            let guard = self.pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, self.pager)?;
            let cell = page.cell(cell_idx)?;
            page.cell_key(&cell, self.pager)?
        };
        if found_key != key {
            let _ = seek_res;
            return Ok(false);
        }
        if is_leaf {
            let is_underflow = self.with_page_mut::<_, bool>(page_no, |page| {
                page.remove_cell(cell_idx)?;
                debug_assert!(page.assert_invariants().is_ok());
                page.is_underflow()
            })?;
            if page_no != self.root_page && is_underflow {
                self.fix_page_underflow(page_no)?;
            }
            return Ok(true);
        }
        // Interior hit. Only index dividers are real entries. Table
        // interiors are routing copies and are never stopped on, so
        // reaching here for a table means corruption, report not found.
        if !is_index {
            return Ok(false);
        }
        let left_child = self.with_page_ref(page_no, |p| Ok(p.cell(cell_idx)?.left_child()))?;
        // Walk to the predecessor, the last cell of the rightmost leaf
        // under the divider left child. The cursor stack already ends at
        // the interior page, so extend it down the right edge. Each
        // interior level parks at its right most slot, the leaf parks at
        // its last cell. That layout is exactly what fix underflow wants.
        // The stack depth is remembered so a full parent below can be
        // unwound back to a shape split_interior understands.
        let base_len = self.cursor.stack.len();
        let mut pred_no = left_child;
        loop {
            let guard = self.pager.get(pred_no)?;
            let page = page_as_ref_with_pager(pred_no, &guard, self.pager)?;
            if page.is_leaf()? {
                let n = page.no_of_cells()?;
                if n == 0 {
                    return Err(SqliteError::Corrupt(
                        "index predecessor leaf is empty".into(),
                    ));
                }
                self.cursor.stack.push(Path::new(pred_no, n - 1, guard));
                break;
            }
            let rmp = page.right_most_ptr()?.ok_or(SqliteError::Corrupt(
                "index interior has no right child".into(),
            ))?;
            let n = page.no_of_cells()?;
            self.cursor.stack.push(Path::new(pred_no, n, guard));
            pred_no = rmp;
        }
        let (pred_page_no, pred_cell_idx) = self.cursor.last_visited_entry_unchecked();
        // Copy the predecessor leaf bytes, then repaint the divider with
        // them. The divider keeps its left child, only the payload moves.
        // A leaf cell is varint plus payload which is exactly what an
        // interior cell carries after its child pointer.
        let pred_bytes = self.with_page_mut(pred_page_no, |leaf| {
            leaf.cell_bytes_as_ref(pred_cell_idx).map(|b| b.to_vec())
        })?;
        let new_divider = Encode::encode_index_interior_cell(left_child, &pred_bytes);
        if self.with_page_mut(page_no, |parent| {
            parent.replace_cell(cell_idx, &new_divider)
        })? == InsertionState::None
        {
            // Parent too full to repaint this divider in place. Unwind
            // the pushed predecessor path plus the divider entry itself,
            // split the parent, then re-seek the divider (untouched, the
            // failed replace wrote nothing) and retry once on fresh space.
            while self.cursor.stack.len() > base_len - 1 {
                self.cursor.stack.pop();
            }
            self.split_interior(page_no)?;
            if self.cursor.seek_for_delete(self.pager, &key)? != SeekResult::Exact {
                return Err(SqliteError::Corrupt(
                    "index divider vanished across parent split".into(),
                ));
            }
            let (page_no2, cell_idx2) = self.cursor.last_visited_entry_unchecked();
            // split_interior may have promoted this very divider one level
            // up. Its left child is then the fresh left half, not the child
            // it carried before, so the repaint has to keep whatever child
            // the relocated cell holds now. Reusing the stale child would
            // orphan the new half and reference the old child twice.
            let child_now =
                self.with_page_ref(page_no2, |p| Ok(p.cell(cell_idx2)?.left_child()))?;
            let new_divider = Encode::encode_index_interior_cell(child_now, &pred_bytes);
            if self.with_page_mut(page_no2, |parent| {
                parent.replace_cell(cell_idx2, &new_divider)
            })? == InsertionState::None
            {
                return Err(SqliteError::Corrupt(
                    "index divider still does not fit after parent split".into(),
                ));
            }
            // Rebuild the predecessor path under the fresh divider for
            // the removal below. The split only redistributed divider
            // keys, so the known leaf cell is still where it was.
            let mut cur = self.with_page_ref(page_no2, |p| Ok(p.cell(cell_idx2)?.left_child()))?;
            let mut depth = 0;
            while cur != pred_page_no {
                depth += 1;
                if depth > 100 {
                    return Err(SqliteError::Corrupt(
                        "predecessor path broken across parent split".into(),
                    ));
                }
                let guard = self.pager.get(cur)?;
                let page = page_as_ref_with_pager(cur, &guard, self.pager)?;
                if page.is_leaf()? {
                    return Err(SqliteError::Corrupt(
                        "predecessor path broken across parent split".into(),
                    ));
                }
                let rmp = page.right_most_ptr()?.ok_or(SqliteError::Corrupt(
                    "index interior has no right child".into(),
                ))?;
                let n = page.no_of_cells()?;
                self.cursor.stack.push(Path::new(cur, n, guard));
                cur = rmp;
            }
            let pred_guard = self.pager.get(pred_page_no)?;
            self.cursor
                .stack
                .push(Path::new(pred_page_no, pred_cell_idx, pred_guard));
        }
        let pred_underflow = self.with_page_mut(pred_page_no, |leaf| {
            leaf.remove_cell(pred_cell_idx)?;
            leaf.is_underflow()
        })?;
        if pred_page_no != self.root_page && pred_underflow {
            self.fix_page_underflow(pred_page_no)?;
        }
        Ok(true)
    }
    /// Collapse an empty interior root: move its single (rightmost) child
    /// into the root page, keeping the root page_no stable so the catalog
    /// stays valid. Leaf roots and roots with >=1 key are left alone.
    /// The orphaned child page is leaked for now (TODO: freelist).
    /// Collapse an empty interior root: move its single (rightmost) child
    /// into the root page, keeping the root page_no stable so the catalog
    /// stays valid. Leaf roots and roots with >=1 key are left alone.
    /// The orphaned child page is leaked for now (TODO: freelist).
    fn collapse_root(&mut self, root_no: PageNo) -> SqliteResult<()> {
        let (n_cells, is_leaf, rmp) = self.with_page_ref(root_no, |page| {
            Ok((page.no_of_cells()?, page.is_leaf()?, page.right_most_ptr()?))
        })?;
        if is_leaf || n_cells > 0 {
            return Ok(());
        }
        let child_no = rmp.ok_or(SqliteError::Internal(
            "empty interior root has no right-most child".into(),
        ))?;
        // Collect the surviving child before overwriting the root.
        let (kind, child_rmp, cells) = self.with_page_mut(child_no, |child| {
            let mut cells = Vec::with_capacity(child.no_of_cells()? as usize);
            for i in 0..child.no_of_cells()? {
                cells.push(child.cell_bytes_as_ref(i)?.to_vec());
            }
            Ok((child.page_type()?, child.right_most_ptr()?, cells))
        })?;
        self.with_page_mut(root_no, |root| {
            root.reset_for_rebuild()?;
            root.set_page_type(kind)?;
            if let Some(rmp) = child_rmp {
                root.set_right_most_ptr(rmp)?;
            }
            for (i, bytes) in cells.iter().enumerate() {
                if root.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "root collapse: child cells do not fit in root".into(),
                    ));
                }
            }
            Ok(())
        })?;
        // The child's content now lives in the root; free the orphan.
        self.deallocate_page(child_no)?;
        Ok(())
    }
    /*
     * THIS FUNCTION RELIES ON THE UNDERFLOW PAGE BEING THE LAST ENTRY
     * IN THE PATH. WE MUST ENSURE THE PATH IS POSITIONED
     * AT THE PAGE CURRENTLY BEING REPAIRED.
     */

    pub fn fix_page_underflow(&mut self, child_page_no: PageNo) -> SqliteResult<()> {
        /*
         * TO FIX UNDERFLOW ON A PAGE
         * WE REQUIRE AT LEAST THE PAGE IT SELF AND ITS PARENT
         */
        if self.cursor.stack.is_empty() {
            return Ok(());
        }
        let _page_no = self.cursor.stack.pop().unwrap().page_no;

        debug_assert_eq!(
            _page_no, child_page_no,
            "The given page ({}) does not match the last page in the path ({})",
            _page_no, child_page_no
        );

        if self.cursor.stack.is_empty() {
            // the popped page was the root.
            // Roots don't underflow: a leaf root with 0 cells is an empty
            // table, an interior root with >=1 key is fine. Only an interior
            // root with 0 keys collapses (its RMP child moves into the root,
            // keeping the root page_no stable so the catalog stays valid).
            if _page_no != self.root_page {
                return Err(SqliteError::Internal(
                    "underflow path popped a non-root page with empty stack".into(),
                ));
            }
            self.collapse_root(_page_no)?;
            return Ok(());
        }

        let (parent_page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();
        let parent_n = self.with_page_ref(parent_page_no, |page| page.no_of_cells())?;
        if parent_n == 0 {
            if parent_page_no != self.root_page {
                return Err(SqliteError::Internal(
                    "non-root interior page with 0 cells".into(),
                ));
            }
            self.collapse_root(parent_page_no)?;
            return Ok(());
        }
        let undeflow_action = self.underflow_planner(parent_page_no, cell_idx, parent_n)?;
        let path = ActivePath::from(self.cursor.stack.as_ref());
        self.try_fix_underflow(undeflow_action, child_page_no, path)?;
        // println!("Underflow Fixed on pageno {}", child_page_no);
        Ok(())
    }

    fn underflow_planner(
        &mut self,
        parent_page_no: PageNo,
        cell_idx: CellIndex,
        parent_cells: u16,
    ) -> SqliteResult<UnderflowAction> {
        if cell_idx == 0 {
            return Ok(UnderflowAction::BorrowRight);
        }
        if cell_idx == parent_cells {
            return Ok(UnderflowAction::BorrowLeft);
        }
        let (left_no, right_no) = self.with_page_ref(parent_page_no, |p| {
            let left = p.cell(cell_idx - 1)?.left_child();
            let right = if cell_idx + 1 < p.no_of_cells()? {
                p.cell(cell_idx + 1)?.left_child()
            } else {
                p.right_most_ptr()?.ok_or(SqliteError::Corrupt(
                    "interior page has no right child".into(),
                ))?
            };
            Ok((left, right))
        })?;
        let left_cells = self.with_page_ref(left_no, |p| p.no_of_cells())?;
        let right_cells = self.with_page_ref(right_no, |p| p.no_of_cells())?;
        if left_cells > right_cells {
            Ok(UnderflowAction::BorrowLeft)
        } else {
            Ok(UnderflowAction::BorrowRight)
        }
    }
}

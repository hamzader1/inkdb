use super::cursor::{BTreeCursor, Path};
use super::{ActivePath, BTree, CellIndex, SplitMetadata, UnderflowAction};
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
    pub(crate) fn try_fix_underflow(
        &mut self,
        underflow_action: UnderflowAction,
        child_page_no: PageNo,
        parent_path: ActivePath,
    ) -> SqliteResult<()> {
        match underflow_action {
            UnderflowAction::BorrowLeft => self.try_borrow_left(child_page_no, parent_path),
            UnderflowAction::BorrowRight => self.try_borrow_right(child_page_no, parent_path),
        }
    }

    fn try_borrow_right(
        &mut self,
        child_page_no: PageNo,
        parent_path: ActivePath,
    ) -> SqliteResult<()> {
        let mut parent_page_guard = self.pager.get_mut(parent_path.page_no)?;
        let mut parent_page = self.page_as_mut(parent_path.page_no, &mut parent_page_guard)?;
        let is_index = parent_page.as_ref()?.is_index()?;
        debug_assert!(
            parent_path.cell_idx < parent_page.no_of_cells()?,
            "Right most pointer has no right sibling"
        );
        let sibling_idx = parent_path.cell_idx + 1;
        let sib_page_no = {
            if sibling_idx < parent_page.no_of_cells()? {
                parent_page.cell(sibling_idx)?.left_child()
            } else {
                parent_page.right_most_ptr()?.unwrap()
            }
        };

        if sib_page_no == child_page_no {
            return Err(SqliteError::Corrupt(
                "borrow right from self, parent holds a duplicate pointer".into(),
            ));
        }
        let mut sibling_page_guard = self.pager.get_mut(sib_page_no)?;
        let mut sibling_page = self.page_as_mut(sib_page_no, &mut sibling_page_guard)?;
        // debug_assert!(
        //     !sibling_page.is_underflow()?,
        //     "Right sibling page (PageNumber: {}) is underflow before borrowing",
        //     sib_page_no
        // );
        let mut current_page_guard = self.pager.get_mut(child_page_no)?;
        let mut current_page = self.page_as_mut(child_page_no, &mut current_page_guard)?;
        let mut all_cells_as_bytes: Vec<Vec<u8>> = Vec::new();
        let mut total_size_in_bytes = 0;
        for i in 0..current_page.no_of_cells()? {
            let bytes = current_page.cell_bytes_as_ref(i)?.to_vec();
            total_size_in_bytes += bytes.len();
            all_cells_as_bytes.push(bytes);
        }
        let current_page_len = current_page.no_of_cells()? as usize;
        for i in 0..sibling_page.no_of_cells()? {
            let bytes = sibling_page.cell_bytes_as_ref(i)?.to_vec();
            total_size_in_bytes += bytes.len();
            all_cells_as_bytes.push(bytes);
        }
        // An index divider is a real entry that lives only in the parent.
        // Pool it with the leaves so merge and redistribute cannot lose it.
        // Table dividers are routing copies, they stay out of the pool.
        // Interior merges pull the separator down with the left right most
        // child so routing for that subtree survives.
        let is_leaf_page = current_page.is_leaf()?;
        if is_index {
            let sep_bytes = parent_page
                .cell_bytes_as_ref(parent_path.cell_idx)?
                .to_vec();
            if is_leaf_page {
                let leaf_sep = sep_bytes[4..].to_vec(); // strip the left page
                total_size_in_bytes += leaf_sep.len(); // add the bytes to the total_size_in_bytes so we can check later if we can merge
                all_cells_as_bytes.insert(current_page_len, leaf_sep);
            } else {
                // Left page right most pointer would be the key of the pulled node
                let left_rmp = current_page.right_most_ptr()?.ok_or(SqliteError::Corrupt(
                    "left interior has no right child".into(),
                ))?;
                let pulled = Encode::encode_index_interior_cell(left_rmp, &sep_bytes[4..]);
                total_size_in_bytes += pulled.len();
                all_cells_as_bytes.insert(current_page_len, pulled);
            }
        } else if !is_leaf_page {
            let left_rmp = current_page.right_most_ptr()?.ok_or(SqliteError::Corrupt(
                "left interior has no right child".into(),
            ))?;
            let rowid = parent_page.cell(parent_path.cell_idx)?.row_id();
            let pulled = Encode::encode_table_interior_cell(left_rmp, rowid);
            total_size_in_bytes += pulled.len();
            all_cells_as_bytes.insert(current_page_len, pulled);
        }

        // Check if they can fit in one page (cells + pointers + header)
        let total_cells = all_cells_as_bytes.len();
        let header_sz = current_page.header_size()? as usize;
        let required = total_size_in_bytes + total_cells * 2 + header_sz;
        if required <= self.pager.usable_size() {
            self.merge(
                all_cells_as_bytes,
                &mut sibling_page,
                parent_path.cell_idx,
                &mut parent_page,
                child_page_no,
            )?;
            return Ok(());
        }

        // The pooled separator above belongs to the merge path, which
        // already returned. The redistribute loops below move the parent
        // separator down on their own, so keeping the pooled copy would
        // insert the same divider twice, side by side. Drop it for
        // interior pages and recount. Leaf pools stay as they are: table
        // leaves never pooled one and index leaves promote theirs.
        if !is_leaf_page {
            let dropped = all_cells_as_bytes.remove(current_page_len);
            total_size_in_bytes -= dropped.len();
        }
        let total_cells = all_cells_as_bytes.len();

        let target = total_size_in_bytes / 2;
        let mut split_at = 0;
        let mut running_size = 0;
        for (i, cell) in all_cells_as_bytes.iter().enumerate() {
            running_size += cell.len();
            if running_size >= target {
                split_at = i + 1;
                break;
            }
        }
        // Byte-split alone can leave a side empty; keep both sides non-empty.
        // Interior path clamps further below (needs a cell to promote).
        if total_cells < 2 {
            return Err(SqliteError::Internal(
                "cannot redistribute: not enough cells".into(),
            ));
        }
        split_at = split_at.clamp(1, total_cells - 1);
        let (new_left_page_cell, new_right_page_cells) = all_cells_as_bytes.split_at_mut(split_at);
        // last cell of the left share
        // this is the one whose row_id becomes the separator
        let separator_index = split_at - 1;

        // let separator_cell = if separator_index < current_page_len {
        //     // it's still one of current_page's original cells
        //     current_page.cell(separator_index as _)?
        // } else {
        //     // it's one of sibling_page's original cells
        //     sibling_page.cell((separator_index - current_page_len) as _)?
        // };
        /*
         * If the page we are rebalancing is a leaf page
         */
        if current_page.is_leaf()? {
            debug_assert!(
                sibling_page.is_leaf()?,
                "The current page is a leaf ({}), while its sibling page is an interior node ({}).",
                current_page.page_no(),
                sib_page_no
            );
            if is_index {
                // Pool already holds left plus separator plus right. Split
                // it, promote the last cell of the left share, keep every
                // other cell in a leaf. Nothing is copied, nothing is lost.
                if total_cells < 3 {
                    return Err(SqliteError::Internal(
                        "cannot redistribute index leaf: not enough cells".into(),
                    ));
                }
                // Both must stay >= 1, otherwise we built an empty page
                // Later we promote left_share.last() to the parent and keeps left_share[..len-1]
                split_at = split_at.clamp(2, total_cells - 1);
                let (left_share, right_share) = all_cells_as_bytes.split_at(split_at);
                let promoted = left_share.last().cloned().ok_or(SqliteError::Internal(
                    "index redistribute left share is empty".into(),
                ))?;
                // We don't include the last cell because its the promoted cell.
                // By doing so, we prevent duplicate the key between parent and child
                let left_cells = &left_share[..left_share.len() - 1];
                current_page.reset_for_rebuild()?;
                for (i, bytes) in left_cells.iter().enumerate() {
                    if current_page.insert_cell(bytes, i as _)? == InsertionState::None {
                        return Err(SqliteError::Internal(
                            "redistribute leaf: left share does not fit".into(),
                        ));
                    }
                }
                sibling_page.reset_for_rebuild()?;
                for (i, bytes) in right_share.iter().enumerate() {
                    if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                        return Err(SqliteError::Internal(
                            "redistribute leaf: right share does not fit".into(),
                        ));
                    }
                }
                let new_bytes = Encode::encode_index_interior_cell(child_page_no, &promoted);
                // See previous comments
                // Remove the current separator to prevent duplicates, since we already have a copy in its childrens
                parent_page.remove_cell(parent_path.cell_idx)?;
                if parent_page.insert_cell(&new_bytes, parent_path.cell_idx)? // Insert the new separator
                    == InsertionState::None
                {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: parent separator does not fit".into(),
                    ));
                }
                return Ok(());
            }
            current_page.reset_for_rebuild()?;
            for (i, bytes) in new_left_page_cell.iter().enumerate() {
                if current_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: left share does not fit".into(),
                    ));
                }
            }
            sibling_page.reset_for_rebuild()?;
            for (i, bytes) in new_right_page_cells.iter().enumerate() {
                if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: right share does not fit".into(),
                    ));
                }
            }
            debug_assert!(
                !current_page.is_underflow()?,
                "Current page still underflows after redistribution \
             (page_no: {}, free_space: {})",
                current_page.page_no(),
                current_page.freespace()?
            );

            debug_assert!(
                !sibling_page.is_underflow()?,
                "Sibling page still underflows after redistribution \
             (page_no: {}, free_space: {})",
                sibling_page.page_no(),
                sibling_page.freespace()?
            );

            let new_bytes = Encode::encode_table_interior_cell(
                child_page_no,
                current_page.cell(separator_index as _)?.row_id(),
            );

            parent_page.remove_cell(parent_path.cell_idx)?;
            if parent_page.insert_cell(&new_bytes, parent_path.cell_idx)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "redistribute leaf: parent separator does not fit".into(),
                ));
            }
        } else {
            // Interior rotation promotes new_right[0] to the parent, so the
            // right share must keep at least 2 cells (promoted + remainder)
            // and the left must grow. split_at == current_page_len would
            // never reach the `i == current_page_len` slot below, so the
            // parent divider would silently not move down and the old
            // right most subtree would be orphaned. Reject it too.
            if new_right_page_cells.len() < 2 || split_at <= current_page_len {
                return Err(SqliteError::Internal(
                    "cannot redistribute interior: split leaves no promotable cell".into(),
                ));
            }

            // let beta =
            //     current_page.cell_key(&current_page.cell((split_at - 1) as _)?, self.pager)?;

            let first_cell_of_right_sibling = if !is_index {
                TableInteriorCell::parse(&new_right_page_cells[0], self.pager.usable_size())
                    .map(BTreeCell::TableInterior)
            } else {
                IndexInteriorCell::parse(&new_right_page_cells[0], self.pager.usable_size())
                    .map(BTreeCell::IndexInterior)
            }?;

            debug_assert_eq!(
                current_page.page_type()?,
                sibling_page.page_type()?,
                "Current page kind ({:?}) does not match sibling page kind ({:?})",
                current_page.page_type()?,
                sibling_page.page_type()?,
            );

            let parent_separator_cell = parent_page.cell(parent_path.cell_idx)?;
            // Sibling keeps its rightmost subtree; save before reset wipes it.
            let sibling_rmp = sibling_page.right_most_ptr()?;

            // Rotation moves the parent separator down into the deficient
            // page and promotes the sibling extreme up. Both entries are
            // relocated, never copied, so the count is conserved. The old
            // code moved the promoted payload down instead of the parent
            // separator, losing one entry and duplicating the other.
            let new_cell_for_curr_page = if !is_index {
                Encode::encode_table_interior_cell(
                    current_page.right_most_ptr()?.unwrap(),
                    parent_separator_cell.row_id(),
                )
            } else {
                // Parent cell is left_child plus varint plus payload.
                // Strip the child, the payload moves down.
                let parent_sep_bytes = parent_page
                    .cell_bytes_as_ref(parent_path.cell_idx)?
                    .to_vec();
                Encode::encode_index_interior_cell(
                    current_page.right_most_ptr()?.unwrap(),
                    &parent_sep_bytes[4..],
                )
            };

            debug_assert!(
                current_page_len < new_left_page_cell.len(),
                "Redistributing Cells has no offect on the underflowed page"
            );
            let mut temp_offset = 0;
            current_page.reset_for_rebuild()?;
            for (i, bytes) in new_left_page_cell.iter().enumerate() {
                if i == current_page_len {
                    if current_page.insert_cell(&new_cell_for_curr_page, i as _)?
                        == InsertionState::None
                    {
                        return Err(SqliteError::Internal(
                            "redistribute interior: parent separator does not fit".into(),
                        ));
                    }
                    temp_offset = 1;
                }
                if current_page.insert_cell(bytes, (i + temp_offset) as _)? == InsertionState::None
                {
                    return Err(SqliteError::Internal(
                        "redistribute interior: left share does not fit".into(),
                    ));
                }
            }

            debug_assert_eq!(
                temp_offset, 1,
                "Separator key was not moved down as expected"
            );
            current_page.set_right_most_ptr(first_cell_of_right_sibling.left_child())?;
            let new_parent_cell = if !is_index {
                Encode::encode_table_interior_cell(
                    child_page_no,
                    first_cell_of_right_sibling.row_id(),
                )
            } else {
                // The promoted sibling extreme moves up. Its payload is
                // the first right share cell, not the cell just moved down.
                Encode::encode_index_interior_cell(child_page_no, &new_right_page_cells[0][4..])
            };
            parent_page.remove_cell(parent_path.cell_idx)?;
            if parent_page.insert_cell(&new_parent_cell, parent_path.cell_idx)?
                == InsertionState::None
            {
                return Err(SqliteError::Internal(
                    "redistribute interior: parent separator does not fit".into(),
                ));
            }
            sibling_page.reset_for_rebuild()?;
            // skip the cell we promote
            for (i, bytes) in new_right_page_cells[1..].iter().enumerate() {
                if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute interior: right share does not fit".into(),
                    ));
                }
            }
            if sibling_page.right_most_ptr()? != sibling_rmp
                && let Some(rmp) = sibling_rmp
            {
                sibling_page.set_right_most_ptr(rmp)?;
            }
            return Ok(());
        }
        Ok(())
    }

    fn try_borrow_left(
        &mut self,
        child_page_no: PageNo,
        parent_path: ActivePath,
    ) -> SqliteResult<()> {
        let mut parent_page_guard = self.pager.get_mut(parent_path.page_no)?;
        let mut parent_page = self.page_as_mut(parent_path.page_no, &mut parent_page_guard)?;
        let is_index = parent_page.as_ref()?.is_index()?;
        debug_assert!(
            parent_path.cell_idx > 0 && parent_path.cell_idx <= parent_page.no_of_cells()?,
            "Left most pointer has no left sibling"
        );
        let sibling_idx = parent_path.cell_idx - 1;
        let sibling_cell = parent_page.cell(sibling_idx)?;
        let sib_page_no = sibling_cell.left_child();

        if sib_page_no == child_page_no {
            return Err(SqliteError::Corrupt(
                "borrow left from self, parent holds a duplicate pointer".into(),
            ));
        }
        let mut sibling_page_guard = self.pager.get_mut(sib_page_no)?;
        let mut sibling_page = self.page_as_mut(sib_page_no, &mut sibling_page_guard)?;
        // debug_assert!(
        //     !sibling_page.is_underflow()?,
        //     "Left sibling page (PageNumber: {}) is underflow before borrowing",
        //     sib_page_no
        // );

        let mut current_page_guard = self.pager.get_mut(child_page_no)?;
        let mut current_page = self.page_as_mut(child_page_no, &mut current_page_guard)?;

        let mut all_cells_as_bytes: Vec<Vec<u8>> = Vec::new();
        let mut total_size_in_bytes = 0;
        // sibling (left, smaller keys) goes FIRST
        for i in 0..sibling_page.no_of_cells()? {
            let bytes = sibling_page.cell_bytes_as_ref(i)?.to_vec();
            total_size_in_bytes += bytes.len();
            all_cells_as_bytes.push(bytes);
        }
        let sibling_len = sibling_page.no_of_cells()? as usize;

        for i in 0..current_page.no_of_cells()? {
            let bytes = current_page.cell_bytes_as_ref(i)?.to_vec();
            total_size_in_bytes += bytes.len();
            all_cells_as_bytes.push(bytes);
        }
        // Same pooling rule as the right side. The divider between the two
        // leaves is a real index entry, so it joins the pool instead of
        // being dropped. Interior separators are pulled down with the left
        // right most child for the same reason.
        let is_leaf_page = current_page.is_leaf()?;
        if is_index {
            let sep_bytes = parent_page.cell_bytes_as_ref(sibling_idx)?.to_vec();
            if is_leaf_page {
                let leaf_sep = sep_bytes[4..].to_vec();
                total_size_in_bytes += leaf_sep.len();
                all_cells_as_bytes.insert(sibling_len, leaf_sep);
            } else {
                let left_rmp = sibling_page.right_most_ptr()?.ok_or(SqliteError::Corrupt(
                    "left interior has no right child".into(),
                ))?;
                let pulled = Encode::encode_index_interior_cell(left_rmp, &sep_bytes[4..]);
                total_size_in_bytes += pulled.len();
                all_cells_as_bytes.insert(sibling_len, pulled);
            }
        } else if !is_leaf_page {
            let left_rmp = sibling_page.right_most_ptr()?.ok_or(SqliteError::Corrupt(
                "left interior has no right child".into(),
            ))?;
            let rowid = parent_page.cell(sibling_idx)?.row_id();
            let pulled = Encode::encode_table_interior_cell(left_rmp, rowid);
            total_size_in_bytes += pulled.len();
            all_cells_as_bytes.insert(sibling_len, pulled);
        }

        let total_cells = all_cells_as_bytes.len();
        let header_sz = current_page.header_size()? as usize;
        let required = total_size_in_bytes + total_cells * 2 + header_sz;
        if required <= self.pager.usable_size() {
            self.merge(
                all_cells_as_bytes,
                &mut current_page,
                sibling_idx,
                &mut parent_page,
                sib_page_no,
            )?;
            return Ok(());
        }

        if !is_leaf_page {
            let dropped = all_cells_as_bytes.remove(sibling_len);
            total_size_in_bytes -= dropped.len();
        }
        let total_cells = all_cells_as_bytes.len();

        let target = total_size_in_bytes / 2;
        let mut split_at = 0;
        let mut running_size = 0;
        for (i, cell) in all_cells_as_bytes.iter().enumerate() {
            running_size += cell.len();
            if running_size >= target {
                split_at = i + 1;
                break;
            }
        }
        if total_cells < 2 {
            return Err(SqliteError::Internal(
                "cannot redistribute: not enough cells".into(),
            ));
        }
        split_at = split_at.clamp(1, total_cells - 1);
        // sibling gets the LEFT half, current_page gets the RIGHT half
        let (new_sibling_cells, new_current_cells) = all_cells_as_bytes.split_at(split_at);

        if current_page.is_leaf()? {
            debug_assert!(
                sibling_page.is_leaf()?,
                "The current page is a leaf ({}), while its sibling page is an interior node ({}).",
                current_page.page_no(),
                sib_page_no
            );
            if is_index {
                // Pool holds sibling plus separator plus current. Promote the
                // last cell of the left share so every entry stays single.
                if total_cells < 3 {
                    return Err(SqliteError::Internal(
                        "cannot redistribute index leaf: not enough cells".into(),
                    ));
                }
                split_at = split_at.clamp(2, total_cells - 1);
                let (left_share, right_share) = all_cells_as_bytes.split_at(split_at);
                let promoted = left_share.last().cloned().ok_or(SqliteError::Internal(
                    "index redistribute left share is empty".into(),
                ))?;
                let left_cells = &left_share[..left_share.len() - 1];
                sibling_page.reset_for_rebuild()?;
                for (i, bytes) in left_cells.iter().enumerate() {
                    if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                        return Err(SqliteError::Internal(
                            "redistribute leaf: left share does not fit".into(),
                        ));
                    }
                }
                current_page.reset_for_rebuild()?;
                for (i, bytes) in right_share.iter().enumerate() {
                    if current_page.insert_cell(bytes, i as _)? == InsertionState::None {
                        return Err(SqliteError::Internal(
                            "redistribute leaf: right share does not fit".into(),
                        ));
                    }
                }
                let new_bytes = Encode::encode_index_interior_cell(sib_page_no, &promoted);
                parent_page.remove_cell(parent_path.cell_idx - 1)?;
                if parent_page.insert_cell(&new_bytes, parent_path.cell_idx - 1)?
                    == InsertionState::None
                {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: parent separator does not fit".into(),
                    ));
                }
                return Ok(());
            }
            let separator_index = split_at - 1;
            let sibling_len = sibling_page.no_of_cells()? as usize;
            let separator_key = if separator_index < sibling_len {
                sibling_page.cell(separator_index as _)?.row_id()
            } else {
                current_page
                    .cell((separator_index - sibling_len) as _)?
                    .row_id()
            };
            let new_bytes = Encode::encode_table_interior_cell(sib_page_no, separator_key);

            sibling_page.reset_for_rebuild()?;
            for (i, bytes) in new_sibling_cells.iter().enumerate() {
                if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: left share does not fit".into(),
                    ));
                }
            }
            current_page.reset_for_rebuild()?;
            for (i, bytes) in new_current_cells.iter().enumerate() {
                if current_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: right share does not fit".into(),
                    ));
                }
            }

            debug_assert!(
                !sibling_page.is_underflow()?,
                "Sibling page still underflows after redistribution (page_no: {}, free_space: {})",
                sibling_page.page_no(),
                sibling_page.freespace()?
            );
            debug_assert!(
                !current_page.is_underflow()?,
                "Current page still underflows after redistribution (page_no: {}, free_space: {})",
                current_page.page_no(),
                current_page.freespace()?
            );

            // separator key = last key of sibling's new share, points to sibling (left child)
            parent_page.remove_cell(parent_path.cell_idx - 1)?;
            if parent_page.insert_cell(&new_bytes, parent_path.cell_idx - 1)?
                == InsertionState::None
            {
                return Err(SqliteError::Internal(
                    "redistribute leaf: parent separator does not fit".into(),
                ));
            }

            return Ok(());
        }

        if split_at < 2 || split_at > total_cells - 1 {
            return Err(SqliteError::Internal(
                "cannot redistribute interior: split leaves no promotable cell".into(),
            ));
        }
        debug_assert_eq!(
            current_page.page_type()?,
            sibling_page.page_type()?,
            "Current page kind ({:?}) does not match sibling page kind ({:?})",
            current_page.page_type()?,
            sibling_page.page_type()?,
        );
        let current_len = current_page.no_of_cells()? as usize;
        let right_final = (total_cells - split_at) + 1;
        if right_final <= current_len {
            return Err(SqliteError::Internal(
                "cannot redistribute interior: split does not grow underflowed page".into(),
            ));
        }
        // Promoted cell = last of left share. Parse before rebuilds overwrite.
        // Table: boundary rowid moves up. Index: the full entry moves up.
        let promoted_bytes = &new_sibling_cells[new_sibling_cells.len() - 1];
        let promoted_cell = if !is_index {
            TableInteriorCell::parse(promoted_bytes, self.pager.usable_size())
                .map(BTreeCell::TableInterior)?
        } else {
            IndexInteriorCell::parse(promoted_bytes, self.pager.usable_size())
                .map(BTreeCell::IndexInterior)?
        };
        let parent_separator_cell = parent_page.cell(sibling_idx)?;
        // Current keeps its rightmost subtree; save before reset wipes it.
        // (Sibling's new RMP is set to the promoted cell's left child below.)
        let current_rmp = current_page.right_most_ptr()?;
        // Parent separator moves down front of right page; its left child is
        // the sibling's old right-most pointer. Index: move the full parent
        // record down (parsed for its payload range first).
        let new_cell_for_right = if !is_index {
            Encode::encode_table_interior_cell(
                sibling_page.right_most_ptr()?.unwrap(),
                parent_separator_cell.row_id(),
            )
        } else {
            // parent cell is left_child + varint+payload; strip left_child
            let parent_cell_bytes = parent_page.cell_bytes_as_ref(sibling_idx)?.to_vec();
            Encode::encode_index_interior_cell(
                sibling_page.right_most_ptr()?.unwrap(),
                &parent_cell_bytes[4..],
            )
        };

        sibling_page.reset_for_rebuild()?;
        for (i, bytes) in new_sibling_cells[..new_sibling_cells.len() - 1]
            .iter()
            .enumerate()
        {
            if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "redistribute interior: left share does not fit".into(),
                ));
            }
        }

        sibling_page.set_right_most_ptr(promoted_cell.left_child())?;
        let new_parent_cell = if !is_index {
            Encode::encode_table_interior_cell(sib_page_no, promoted_cell.row_id())
        } else {
            // promoted_bytes is left_child + varint+payload; strip left_child
            Encode::encode_index_interior_cell(sib_page_no, &promoted_bytes[4..])
        };
        parent_page.remove_cell(sibling_idx)?;
        if parent_page.insert_cell(&new_parent_cell, sibling_idx)? == InsertionState::None {
            return Err(SqliteError::Internal(
                "redistribute interior: parent separator does not fit".into(),
            ));
        }

        //
        let leftover_sib_in_right = sibling_len
            .saturating_sub(split_at)
            .min(new_current_cells.len());
        current_page.reset_for_rebuild()?;
        let mut slot = 0usize;
        for bytes in &new_current_cells[..leftover_sib_in_right] {
            if current_page.insert_cell(bytes, slot as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "redistribute interior: right share does not fit".into(),
                ));
            }
            slot += 1;
        }
        if current_page.insert_cell(&new_cell_for_right, slot as _)? == InsertionState::None {
            return Err(SqliteError::Internal(
                "redistribute interior: parent separator does not fit".into(),
            ));
        }
        slot += 1;
        for bytes in &new_current_cells[leftover_sib_in_right..] {
            if current_page.insert_cell(bytes, slot as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "redistribute interior: right share does not fit".into(),
                ));
            }
            slot += 1;
        }
        if current_page.right_most_ptr()? != current_rmp
            && let Some(rmp) = current_rmp
        {
            current_page.set_right_most_ptr(rmp)?;
        }

        Ok(())
    }

    fn merge(
        &mut self,
        all_cells_as_bytes: Vec<Vec<u8>>,
        right_page: &mut BTreePageMut,
        separator_index: CellIndex,
        parent_page: &mut BTreePageMut,
        abandoned: PageNo,
    ) -> SqliteResult<()> {
        if right_page.page_no() == abandoned {
            return Err(SqliteError::Corrupt("merge of a page into itself".into()));
        }

        let merged_rmp = right_page.right_most_ptr()?;
        right_page.reset_for_rebuild()?;
        for (i, bytes) in all_cells_as_bytes.iter().enumerate() {
            if right_page.insert_cell(bytes, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "merge: combined cells do not fit in one page".into(),
                ));
            }
        }
        if right_page.right_most_ptr()? != merged_rmp
            && let Some(rmp) = merged_rmp
        {
            right_page.set_right_most_ptr(rmp)?;
        }
        parent_page.remove_cell(separator_index)?;
        if parent_page.is_underflow()? {
            let parent_no = parent_page.page_no();
            self.fix_page_underflow(parent_no)?;
        }
        // All content now lives in right_page; the left page is garbage.
        // Free it only on the success path so a failed rebalance never
        // frees a page the tree still references.
        self.deallocate_page(abandoned)?;

        Ok(())
    }
}

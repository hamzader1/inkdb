use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::pager::PageNo;
use crate::storage::page::{BTreePage, InsertionState, PageMut, PageRef};
use crate::vfs::Vfs;

use super::insert::{BTree, guard_not_mutable};
use super::kind::{
    AnyPage, IndexInterior, IndexLeaf, PageKind, TableInterior, TableLeaf, TypedPage,
};
use super::policy::RebalancePolicy;
use super::typed_mut::parse_ref;
use crate::storage::btree::CellIndex;

impl<'a, V: Vfs> BTree<'a, V> {
    pub(crate) fn rebalance<K: RebalancePolicy>(
        &mut self,
        left_page: PageNo,
        right_page: PageNo,
        parent_no: PageNo,
        divider_idx: CellIndex,
    ) -> SqliteResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();

        let (left_cells, left_rmp, header_size) = {
            let guard = self.pager.get(left_page)?;
            let page = parse_ref::<K, V>(left_page, &guard, self.pager)?;
            (
                stage(&page)?,
                page.right_most_ptr()?,
                page.header_size()? as usize,
            )
        };
        let (right_cells, right_rmp) = {
            let guard = self.pager.get(right_page)?;
            let page = parse_ref::<K, V>(right_page, &guard, self.pager)?;
            (stage(&page)?, page.right_most_ptr()?)
        };
        let sep_cell = {
            let guard = self.pager.get(parent_no)?;
            let parent = PageRef::new(parent_no, page_size, usable, guard.bytes())?;
            parent.cell_bytes_as_ref(divider_idx)?.to_vec()
        };

        let mut pool: Vec<Vec<u8>> = left_cells;
        if let Some(pulled) = K::pull_down(&sep_cell, left_rmp, usable)? {
            pool.push(pulled);
        }
        pool.extend(right_cells);

        let bytes: usize = pool.iter().map(|cell| cell.len()).sum();
        let required = bytes + pool.len() * 2 + header_size;

        if required <= usable {
            return self.merge::<K>(
                &pool,
                left_page,
                right_page,
                right_rmp,
                parent_no,
                divider_idx,
            );
        }
        self.redistribute::<K>(
            &pool,
            left_page,
            right_page,
            left_rmp,
            right_rmp,
            parent_no,
            divider_idx,
            &sep_cell,
        )
    }

    fn merge<K: RebalancePolicy>(
        &mut self,
        pool: &[Vec<u8>],
        left_page: PageNo,
        right_page: PageNo,
        rmp: Option<PageNo>,
        parent_no: PageNo,
        divider_idx: CellIndex,
    ) -> SqliteResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();

        {
            let mut guard = self.pager.get_mut(right_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page =
                TypedPage::<&mut [u8], K>::parse_mut(right_page, page_size, usable, bytes)?;
            page.reset_for_rebuild()?;
            for (i, cell) in pool.iter().enumerate() {
                if page.insert_cell(cell, i as CellIndex)? == InsertionState::None {
                    return Err(SqliteError::Internal(format!(
                        "merge: combined cells do not fit in page {right_page}"
                    )));
                }
            }
            if let Some(rmp) = rmp {
                page.set_right_most_ptr(rmp)?;
            }
        }

        let parent_shrank = {
            let mut guard = self.pager.get_mut(parent_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut parent = PageMut::new(parent_no, page_size, usable, bytes)?;
            parent.remove_cell(divider_idx)?;
            parent.is_underflow()?
        };

        self.pager.dealloc(left_page)?;

        if parent_shrank && parent_no != self.root_page {
            self.fix_page_underflow(parent_no)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn redistribute<K: RebalancePolicy>(
        &mut self,
        pool: &[Vec<u8>],
        left_page: PageNo,
        right_page: PageNo,
        left_rmp: Option<PageNo>,
        rmp: Option<PageNo>,
        parent_no: PageNo,
        divider_idx: CellIndex,
        sep_cell: &[u8],
    ) -> SqliteResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();

        let n = pool.len();
        if n < 3 {
            return Err(SqliteError::Internal(
                "redistribute: not enough cells to spread over two pages".into(),
            ));
        }
        let total: usize = pool.iter().map(|cell| cell.len()).sum();
        let target = total / 2;
        let mut split_at = 0;
        let mut running = 0;
        for (i, cell) in pool.iter().enumerate() {
            running += cell.len();
            if running >= target {
                split_at = i + 1;
                break;
            }
        }
        let split_at = split_at.clamp(2, n - 1);
        let (left_share, right_share) = pool.split_at(split_at);

        let plan = K::redistribute(
            left_share,
            right_share,
            left_page,
            left_rmp,
            sep_cell,
            usable,
        )?;

        {
            let mut guard = self.pager.get_mut(right_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page =
                TypedPage::<&mut [u8], K>::parse_mut(right_page, page_size, usable, bytes)?;
            page.reset_for_rebuild()?;
            for (i, cell) in right_share.iter().enumerate() {
                if page.insert_cell(cell, i as CellIndex)? == InsertionState::None {
                    return Err(SqliteError::Internal(format!(
                        "redistribute: right share does not fit in page {right_page}"
                    )));
                }
            }
            if let Some(rmp) = rmp {
                page.set_right_most_ptr(rmp)?;
            }
        }
        {
            let mut guard = self.pager.get_mut(left_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page =
                TypedPage::<&mut [u8], K>::parse_mut(left_page, page_size, usable, bytes)?;
            page.reset_for_rebuild()?;
            let keep = if plan.drop_left_last {
                &left_share[..left_share.len() - 1]
            } else {
                left_share
            };
            for (i, cell) in keep.iter().enumerate() {
                if page.insert_cell(cell, i as CellIndex)? == InsertionState::None {
                    return Err(SqliteError::Internal(format!(
                        "redistribute: left share does not fit in page {left_page}"
                    )));
                }
            }
            if let Some(rmp) = plan.left_rmp {
                page.set_right_most_ptr(rmp)?;
            }
        }
        {
            let mut guard = self.pager.get_mut(parent_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut parent = PageMut::new(parent_no, page_size, usable, bytes)?;
            if parent.replace_cell(divider_idx, &plan.parent_cell)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "redistribute: parent separator does not fit".into(),
                ));
            }
        }
        Ok(())
    }
}

impl<'a, V: Vfs> BTree<'a, V> {
    pub(crate) fn fix_page_underflow(&mut self, child: PageNo) -> SqliteResult<()> {
        let Some(child_path) = self.cursor.stack.pop() else {
            return Ok(());
        };
        let popped = child_path.page_no;
        drop(child_path);
        debug_assert_eq!(
            popped, child,
            "underflow repair: path does not end at the page"
        );

        let Some(parent_path) = self.cursor.stack.last() else {
            return self.collapse_root(child);
        };
        let parent_no = parent_path.page_no;
        let slot = parent_path.cell_idx;

        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();

        let (left_page, right_page, divider_idx) = {
            let guard = self.pager.get(parent_no)?;
            let parent = PageRef::new(parent_no, page_size, usable, guard.bytes())?;
            let n = parent.no_of_cells()?;
            if n == 0 {
                return self.collapse_root(parent_no);
            }
            let mut children = Vec::with_capacity(n as usize + 1);
            for i in 0..n {
                let at = parent.cell_ptr(i)? as usize;
                let b = parent.bytes();
                children.push(u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]));
            }
            children.push(parent.right_most_ptr()?.ok_or_else(|| {
                SqliteError::Corrupt("interior page has no right-most child".into())
            })?);
            if slot == 0 {
                (child, children[1], 0)
            } else {
                (children[slot as usize - 1], child, slot - 1)
            }
        };

        let guard = self.pager.get(child)?;
        match AnyPage::parse(child, page_size, usable, guard.bytes())? {
            AnyPage::TableLeaf(_) => {
                self.rebalance::<TableLeaf>(left_page, right_page, parent_no, divider_idx)
            }
            AnyPage::IndexLeaf(_) => {
                self.rebalance::<IndexLeaf>(left_page, right_page, parent_no, divider_idx)
            }
            AnyPage::TableInterior(_) => {
                self.rebalance::<TableInterior>(left_page, right_page, parent_no, divider_idx)
            }
            AnyPage::IndexInterior(_) => {
                self.rebalance::<IndexInterior>(left_page, right_page, parent_no, divider_idx)
            }
        }
    }

    fn collapse_root(&mut self, root_no: PageNo) -> SqliteResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();

        let (n, is_leaf, rmp) = {
            let guard = self.pager.get(root_no)?;
            let page = PageRef::new(root_no, page_size, usable, guard.bytes())?;
            (
                page.no_of_cells()?,
                page.page_type()?.is_leaf(),
                page.right_most_ptr()?,
            )
        };
        if is_leaf || n > 0 {
            return Ok(());
        }
        let child_no = rmp.ok_or_else(|| {
            SqliteError::Internal("empty interior root has no right-most child".into())
        })?;

        let (cells, child_rmp, child_type) = {
            let guard = self.pager.get(child_no)?;
            let page = PageRef::new(child_no, page_size, usable, guard.bytes())?;
            let count = page.no_of_cells()?;
            let mut cells = Vec::with_capacity(count as usize);
            for i in 0..count {
                cells.push(page.cell_bytes_as_ref(i)?.to_vec());
            }
            (cells, page.right_most_ptr()?, page.page_type()?)
        };

        {
            let mut guard = self.pager.get_mut(root_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = BTreePage::<&mut [u8]>::new_from_raw_bytes(
                root_no, child_type, bytes, page_size, usable,
            )?;
            for (i, cell) in cells.iter().enumerate() {
                if page.insert_cell(cell, i as CellIndex)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "root collapse: child cells do not fit in the root".into(),
                    ));
                }
            }
            if let Some(rmp) = child_rmp {
                page.set_right_most_ptr(rmp)?;
            }
        }
        self.pager.dealloc(child_no)?;
        Ok(())
    }
}

fn stage<K: PageKind>(page: &TypedPage<&[u8], K>) -> SqliteResult<Vec<Vec<u8>>> {
    let n = page.no_of_cells()?;
    let mut cells = Vec::with_capacity(n as usize);
    for i in 0..n {
        cells.push(page.cell_bytes_as_ref(i)?.to_vec());
    }
    Ok(cells)
}

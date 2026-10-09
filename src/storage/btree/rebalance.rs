use crate::InkResult;
use crate::errors::{CorruptError, InkError};
use crate::pager::pager::PageNo;
use crate::storage::page::{BTreePage, InsertionState, PageMut, PageRef};
use crate::vfs::Vfs;

use super::insert::guard_not_mutable;
use super::kind::{
    AnyPage, IndexInterior, IndexLeaf, PageKind, TableInterior, TableLeaf, TypedPage,
};
use super::ops::RebalanceOps;
use super::tree::BTree;
use super::typed_mut::parse_ref;
use crate::storage::btree::CellIndex;

impl<'a, V: Vfs> BTree<'a, V> {
    /// Put two neighbouring pages back in order after one of them lost a cell.
    ///
    /// Both pages are emptied and their cells gathered into one run, with the
    /// parent divider brought down into the middle of it. When the run fits in
    /// one page the two are merged into the right one; when it does not, the run
    /// is shared out across both.
    ///
    /// # Example
    /// Let's work on this tree
    /// ```text
    ///                     +-----------+
    ///                    /|  40, 80   |\
    ///                   / +-----------+ \
    ///         /--------/        |        \---------\
    ///        /                  |                   \
    ///       v                   v                    v
    /// +-----------+     +---------------+    +---------------+
    /// |10, 20, 30 |     |42, 51, 60, 70 |    |82, 91, 97, 97 |
    /// +-----------+     +---------------+    +---------------+
    /// PageNo(5)       PageNo(7) ^                    RMP
    ///                           |
    ///                      Page we are
    ///                      working for
    /// ```
    /// We need to rebalance PageNo(3).
    /// We get here from [`BTree::fix_page_underflow`] after deciding which sibling
    /// to merge with.
    /// We first gather the cells from the left page (PageNo(5)). Critically, we
    /// must move the separator cell down before gathering the cells from the right
    /// page. You'll see why when we reach the rebalance phase.
    ///
    /// the cells now should look like
    /// ```text
    /// +---------------------------------+
    /// | 10, 20, 30, 40, 42, 51, 60, 70  |
    /// +--------------+------------------+
    /// |          ^   |  |               ^
    /// +----------+   |  +---------------+
    ///    Left cells  |    Right cells
    ///                |
    ///                v
    ///
    ///               Pulled
    ///             down cell
    /// ```
    /// Now we have two possibilities:
    /// 1. The merged cells fit on a single page. If so, we call the merge function
    ///    and we are done, follow [`BTree::merge`]
    /// 2. They do not fit, so we need to handle it the hard way by redistributing, follow [`BTree::redistribute]
    /// # Errors
    /// Whatever reading the pages or the parent reports.
    pub(crate) fn rebalance<K: RebalanceOps>(
        &mut self,
        left_page: PageNo,
        right_page: PageNo,
        parent_no: PageNo,
        divider_idx: CellIndex,
    ) -> InkResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();

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
            let parent = PageRef::new(parent_no, page_size, usable, header_len, guard.bytes())?;
            parent.cell_bytes_as_ref(divider_idx)?.to_vec()
        };

        let mut pool: Vec<Vec<u8>> = left_cells;
        if let Some(pulled) = K::pull_down(&sep_cell, left_rmp, usable)? {
            // Pull down the separator based on the page policy.
            pool.push(pulled);
        }
        pool.extend(right_cells);

        let bytes: usize = pool.iter().map(|cell| cell.len()).sum();
        /*
         * 1. The total size of the cell data in bytes.
         * 2. The size of the cell pointer array. Each cell pointer is 2 bytes (pool.len() multiplied by 2).
         * 3. The size of the page header.
         */
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

    /// Put the gathered cells into the right page and drop the left one.
    ///
    /// The parent loses the divider that used to separate the two, and when that
    /// leaves the parent short of cells it is repaired in turn, which is how a
    /// loss can travel up the tree.
    ///
    /// Phase 2, Path 1: Merge the cells because they fit on a single page.
    /// We prefer to keep the right page for a specific reason. The right page
    /// might be the rightmost pointer (RMP) of a parent page. If we kept the left
    /// page and discarded the right one, we would have to navigate to the parent
    /// and update its RMP to point to the left page instead, since an interior
    /// page must always have an RMP.
    /// Anyway, we receive the merged cells as the following
    /// ```text
    /// +---------------------------------+
    /// | 10, 20, 30, 40, 42, 51, 60, 70  |
    /// +--------------+------------------+
    /// |          ^   |  |               ^
    /// +----------+   |  +---------------+
    ///    Left cells  |    Right cells
    ///                |
    ///                v
    ///              Pulled
    ///            down cell
    ///
    /// ```
    /// After filling the right page with all those cells and setting its RMP,
    /// we need to remove the separator cell from the parent. The parent still
    /// holds the cell that was pulled down, which in this case is `[40]`.
    /// This is fine for table B trees because we do not read data from interior
    /// cells, but for index B trees, it would duplicate the entry.
    /// Once we remove that cell from the parent, we deallocate the left page
    /// so it can be reused later.
    ///
    /// After the merge the tree should look like:
    /// ```text
    ///                           +-----------+
    ///                          /|    80     |\
    ///                         / +-----------+ \
    ///               /--------/                 \---------\
    ///              v                                      v
    /// +-------------------------------+           +---------------+
    /// |10, 20, 30, 40, 42, 51, 60, 70 |           |82, 91, 97, 97 |
    /// +-------------------------------+           +---------------+
    ///                                                     RMP
    /// ```
    ///
    fn merge<K: RebalanceOps>(
        &mut self,
        pool: &[Vec<u8>],
        left_page: PageNo,
        right_page: PageNo,
        rmp: Option<PageNo>,
        parent_no: PageNo,
        divider_idx: CellIndex,
    ) -> InkResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();

        {
            let mut guard = self.pager.get_mut(right_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = TypedPage::<&mut [u8], K>::parse_mut(
                right_page, page_size, usable, header_len, bytes,
            )?;
            page.reset_for_rebuild()?;
            for (i, cell) in pool.iter().enumerate() {
                if page.insert_cell(cell, i as CellIndex)? == InsertionState::None {
                    return Err(InkError::InternalFmt(format!(
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
            let mut parent = PageMut::new(parent_no, page_size, usable, header_len, bytes)?;
            parent.remove_cell(divider_idx)?;
            parent.is_underflow()?
        };

        self.pager.dealloc(left_page)?;

        if parent_shrank && parent_no != self.root_page {
            self.fix_page_underflow(parent_no)?;
        }
        Ok(())
    }

    /// Share the gathered cells across both pages when they cannot fit on a
    /// single page.
    ///
    /// The cells are split in half by byte size rather than by count, so both
    /// pages end up similarly full even when the cells differ significantly in
    /// length. The split point is kept away from either end to ensure both pages
    /// retain at least two cells, which is required for a later split.
    ///
    /// Phase 2, Path 2: The cells do not fit on a single page, so we need to
    /// redistribute them.
    ///
    /// ### NOTE
    /// Each [`BTreePage`] has its own cell encoding format.
    ///
    /// [1] [`TableLeaf`]: We take the last cell from the left partition and use it
    /// to form an interior cell. We set `drop_left_last` to `false` because
    /// interior table keys are used for navigation only.
    ///
    /// [2] [`IndexLeaf`]: The process is similar to [`TableLeaf::redistribute`],
    /// but we set `drop_left_last` to `true` because `IndexLeaf` and `IndexTable`
    /// cells contain actual entries. We remove that cell from the left page and
    /// move it to the parent.
    ///
    /// [3] [`TableInterior`]: The process is similar to [`IndexLeaf`]. We remove
    /// the last cell from the left page so the parent can take it, preventing
    /// duplication.
    ///
    /// [4] [`IndexInterior`]: The process is the same as for [`TableInterior`],
    /// but the cell encoding differs.
    ///
    /// Once we get the result from one of the four cases above as a
    /// [`super::ops::Redistribute`] struct, we reset the right page, insert the
    /// right partition's cells, and set its RMP. We do the same for the left page,
    /// taking `drop_left_last` into account. Finally, we replace the parent cell
    /// with the cell returned by the redistribution call.
    ///
    /// # Example
    /// This is an example of an underflowing table leaf page.
    /// ### Tree in underflow state.
    /// ```text
    ///                                +-----------+
    ///                               /|  65, 80   |\
    ///                              / +-----------+ \
    ///               /-------------/        |        \---------\
    ///              /                       |                   \
    ///             v              underflow v                    v
    /// +----------------------+         +-------+        +---------------+
    /// |10, 20, 30, 40, 50, 61|         |   70  |        |82, 91, 97, 97 |
    /// +----------------------+         +-------+        +---------------+
    ///                                                           RMP
    /// ```
    /// ### Merge cells
    /// ```text
    /// +---------------------------------+
    /// | 10, 20, 30, 40, 50, 61, 65, 70  |
    /// +---------------------------------+
    /// ```
    /// ### Split the merged cells
    /// ```text
    ///
    ///      Left share                   Right share
    /// +------------------+           +-----------------+
    /// |  10, 20, 30, 40  |           | 50, 61, 65, 70  |
    /// +------------------+           +-----------------+
    /// ```
    /// Since this is a leaf page, we do not drop the last cell from the left partition.
    /// ### Redistribute and final tree
    /// ```text
    ///                           +-----------+
    ///                          /|  40, 80   |\
    ///                         / +-----------+ \
    ///          /-------------/        |        \---------\
    ///         /                       |                   \
    ///        v                        v                    v
    /// +------------------+    +-----------------+  +---------------+
    /// |  10, 20, 30, 40  |    | 50, 61, 65, 70  |  |82, 91, 97, 97 |
    /// +------------------+    +-----------------+  +---------------+
    ///                                                      RMP
    /// ```
    // Sorry, Clippy. I had to do it
    #[allow(clippy::too_many_arguments)]
    fn redistribute<K: RebalanceOps>(
        &mut self,
        pool: &[Vec<u8>],
        left_page: PageNo,
        right_page: PageNo,
        left_rmp: Option<PageNo>,
        rmp: Option<PageNo>,
        parent_no: PageNo,
        divider_idx: CellIndex,
        sep_cell: &[u8],
    ) -> InkResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();

        let n = pool.len();
        // This is important, to redistribute correctly we need at least three cells
        // [one_left, pulled_down, one_right]
        if n < 3 {
            return Err(InkError::Internal(
                "redistribute: not enough cells to spread over two pages",
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
            let mut page = TypedPage::<&mut [u8], K>::parse_mut(
                right_page, page_size, usable, header_len, bytes,
            )?;
            page.reset_for_rebuild()?;
            for (i, cell) in right_share.iter().enumerate() {
                if page.insert_cell(cell, i as CellIndex)? == InsertionState::None {
                    return Err(InkError::InternalFmt(format!(
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
            let mut page = TypedPage::<&mut [u8], K>::parse_mut(
                left_page, page_size, usable, header_len, bytes,
            )?;
            page.reset_for_rebuild()?;
            let keep = if plan.drop_left_last {
                &left_share[..left_share.len() - 1]
            } else {
                left_share
            };
            for (i, cell) in keep.iter().enumerate() {
                if page.insert_cell(cell, i as CellIndex)? == InsertionState::None {
                    return Err(InkError::InternalFmt(format!(
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
            let mut parent = PageMut::new(parent_no, page_size, usable, header_len, bytes)?;
            if parent.replace_cell(divider_idx, &plan.parent_cell)? == InsertionState::None {
                return Err(InkError::Internal(
                    "redistribute: parent separator does not fit",
                ));
            }
        }
        Ok(())
    }
}

impl<'a, V: Vfs> BTree<'a, V> {
    /// Fix a page that has lost so many cells that it is barely worth keeping.
    ///
    /// The page is rebalanced with one of its neighbours as they appear in the
    /// parent, so index entries keep their order. With no cells around it to
    /// borrow, a parent that is short of them is dealt with in turn, and a root
    /// with no cells left is collapsed into its only child.
    ///
    /// This is phase 1. We decide which sibling to merge with or redistribute.
    pub(crate) fn fix_page_underflow(&mut self, child: PageNo) -> InkResult<()> {
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
        let header_len = self.pager.header_len();

        let (left_page, right_page, divider_idx) = {
            let guard = self.pager.get(parent_no)?;
            let parent = PageRef::new(parent_no, page_size, usable, header_len, guard.bytes())?;
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
            children.push(parent.right_most_ptr()?.ok_or({
                InkError::Corrupt(CorruptError::MissingRightMostChild { page: parent_no })
            })?);
            // There is no right sibling, we are the left most.
            if slot == 0 {
                (child, children[1], 0)
            } else {
                (children[slot as usize - 1], child, slot - 1)
            }
        };

        let guard = self.pager.get(child)?;
        match AnyPage::parse(child, page_size, usable, header_len, guard.bytes())? {
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

    /// Replace a root that has no cells left with its only child, and free the
    /// root page.
    ///
    /// Nothing is done to a root that still has cells, or to a root that is a
    /// leaf, since an empty tree is still a tree. This is the only place a tree
    /// gets shorter.
    /// # Example
    /// Let's say we have deleted the root's left subtree. The current root is:
    /// ```text
    ///                         Root
    ///                     +-----------+     After delete 80
    ///                    /|    80     |\   the root is empty
    ///                   / +-----------+ \
    ///                  /                 \---------\
    ///         /-------/                             \
    ///        /                                       v
    ///       v                                +---------------+
    /// +-----------+                          |82, 91, 97, 97 |
    /// |   Empty   |                          +---------------+
    /// +-----------+                                  RMP
    /// ```
    ///
    /// If this happens, we collapse the root.
    /// The tree becomes:
    /// ```text
    /// +------------------------------------+                  +-----------------------+
    /// |                                    |                  |                       |
    /// |     Root                           |                  |                       |
    /// | +-----------+                      |                  |                       |
    /// | |           |\                     |                  |     Root (leaf)       |
    /// | +-----------+ \                    |                  |                       |
    /// |                \                   |                  |   +---------------+   |
    /// |                 \-------\          | -------------->  |   |82, 91, 97, 97 |   |
    /// |                          \         |                  |   +---------------+   |
    /// |                           v        |                  |                       |
    /// |                   +---------------+|                  |                       |
    /// |                   |82, 91, 97, 97 ||                  |                       |
    /// |                   +---------------+|                  |                       |
    /// |                           RMP      |                  |                       |
    /// |                                    |                  |                       |
    /// +------------------------------------+                  +-----------------------+
    /// ```
    fn collapse_root(&mut self, root_no: PageNo) -> InkResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();

        let (n, is_leaf, rmp) = {
            let guard = self.pager.get(root_no)?;
            let page = PageRef::new(root_no, page_size, usable, header_len, guard.bytes())?;
            (
                page.no_of_cells()?,
                page.page_type()?.is_leaf(),
                page.right_most_ptr()?,
            )
        };
        if is_leaf || n > 0 {
            return Ok(());
        }
        let child_no = rmp.ok_or(InkError::Internal(
            "empty interior root has no right-most child",
        ))?;

        let (cells, child_rmp, child_type) = {
            let guard = self.pager.get(child_no)?;
            let page = PageRef::new(child_no, page_size, usable, header_len, guard.bytes())?;
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
                root_no, child_type, bytes, page_size, usable, header_len,
            )?;
            for (i, cell) in cells.iter().enumerate() {
                if page.insert_cell(cell, i as CellIndex)? == InsertionState::None {
                    return Err(InkError::Internal(
                        "root collapse: child cells do not fit in the root",
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

/// Copy a page cells out of it, so they can be laid out again elsewhere.
fn stage<K: PageKind>(page: &TypedPage<&[u8], K>) -> InkResult<Vec<Vec<u8>>> {
    let n = page.no_of_cells()?;
    let mut cells = Vec::with_capacity(n as usize);
    for i in 0..n {
        cells.push(page.cell_bytes_as_ref(i)?.to_vec());
    }
    Ok(cells)
}

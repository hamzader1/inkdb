use crate::errors::{CorruptError, InkError};
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::btree::CellIndex;
use crate::storage::page::{
    BTreePage, InsertionState, compute_index_local_payload_size, compute_table_local_payload_size,
};
use crate::vfs::Vfs;
use crate::{InkResult, MemCursor};

use super::cursor::BTreeCursor;
use super::kind::{AnyPage, IndexLeaf, TableLeaf, TypedPage};
use super::ops::{CellOps, Consumed, Divider, InteriorOps, LeafKind, ParentSlot, Split};
use super::tree::BTree;
use super::typed_mut::{AnyPageMut, parse_ref};

impl<'a, V: Vfs> BTree<'a, V> {
    pub fn new(root_page: PageNo, pager: &'a mut Pager<V>) -> Self {
        Self {
            root_page,
            pager,
            cursor: BTreeCursor::new(root_page),
        }
    }
}

impl<'a, V: Vfs> BTree<'a, V> {
    /// Store a value under a key, taking the cell bytes as they come.
    pub fn insert_value(&mut self, key: &Value, cell_bytes: &[u8]) -> InkResult<()> {
        self.insert_cell(key, cell_bytes.to_vec())
    }

    /// Store a value under a key.
    ///
    /// A payload too long for one page is split off onto overflow pages first,
    /// then the cell goes into the leaf a seek lands on. When it does not fit
    /// there, the page is split and the halves are placed.
    pub fn insert_cell(&mut self, key: &Value, mut cell_bytes: Vec<u8>) -> InkResult<()> {
        self.cursor.seek(self.pager, key)?;
        self.fix_overlow(&mut cell_bytes)?;
        let (page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();

        let fits = {
            let mut guard = self.pager.get_mut(page_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let page_size = self.pager.page_size();
            let usable = self.pager.usable_size();
            let header_len = self.pager.header_len();
            let mut page = AnyPageMut::parse(page_no, page_size, usable, header_len, bytes)?;
            match &mut page {
                AnyPageMut::TableLeaf(p) => {
                    p.insert_cell(&cell_bytes, cell_idx)? == InsertionState::Inserted
                }
                AnyPageMut::IndexLeaf(p) => {
                    p.insert_cell(&cell_bytes, cell_idx)? == InsertionState::Inserted
                }
                _ => return Err(not_a_leaf(page_no)),
            }
        };
        if fits {
            return Ok(());
        }

        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();
        let guard = self.pager.get(page_no)?;
        match AnyPage::parse(page_no, page_size, usable, header_len, guard.bytes())? {
            AnyPage::TableLeaf(_) => self.split_then_place::<TableLeaf>(page_no, key, &cell_bytes),
            AnyPage::IndexLeaf(_) => self.split_then_place::<IndexLeaf>(page_no, key, &cell_bytes),
            _ => Err(not_a_leaf(page_no)),
        }
    }

    /// Split the leaf, put the divider in the parent, and place the cell in
    /// whichever half it belongs to.
    ///
    /// A split at the root grows a new root, since there is no parent to take
    /// the divider.
    ///
    /// # Phase 1: Split the leaf
    /// [`BTree::split_page`] splits the leaf page into two leaves.
    /// The left page is the original page being split, and the right page is
    /// a newly allocated page.
    /// The function returns a [`super::ops::Split`] struct containing the data
    /// needed for the next operation, which is to insert the divider into the parent.
    ///
    /// The tree (of table leaf) after the split_page stage would be:
    /// ```text
    ///                             +-----------+
    ///                            /|  40, 80   |\
    ///                           / +-----------+ \
    ///            /-------------/        |        \---------\
    ///           /                       |                   \
    ///          v                        v                    v
    /// +----------------+        +-----------------+  +---------------+
    /// | 10, 20, 22, 25 |        | 50, 61, 65, 70  |  |82, 91, 97, 97 |
    /// +----------------+        +-----------------+  +---------------+
    ///      PageNo(4)                                       RMP
    ///
    ///
    ///    Leaked page +-----------------+
    ///     for now.   |29 30, 33, 39 40 | PageNo(9)
    ///                +-----------------+
    /// ```
    /// The function returns:
    /// `left_page_no`: The page number of the left page, which is `4` in this example.
    /// `right_page_no`: The page number of the newly allocated right page, which is `9`.
    /// `divider`: The cell to be promoted to the parent, which has the key [`25`].
    /// `right_bound`: The maximum key in the right page, which is [`40`] in this example.S
    ///
    /// We pop the current page (the left page, PageNo(4)) and then pop its parent.
    /// If there is no parent, the current page was the root, so we need to create
    /// a new interior root, insert the left page at index 0, and set its RMP to
    /// point to the right page.
    ///
    /// If there is a parent, we need to insert the new cell into it.
    /// However, insertion is a little tricky because we must account for the
    /// parent and grandparent overflowing as well. We need to recursively fix
    /// any overflows and keep track of the split pages so we know where to
    /// insert the cell, even after one or more parent splits.
    ///
    /// The other tricky part is determining whether the split page was the RMP
    /// of its parent. If it was, we need to update the parent's RMP and insert
    /// only the left cell, since the right page is represented by the RMP.
    ///
    /// If the split page was not the RMP, we replace the cell at the split page's
    /// index with the new left cell and insert the right page as a new cell.
    ///
    /// # Example
    /// ### Case 1: The split page was the RMP
    /// ```text
    ///                          +-----------+
    ///                         /|  40, 80   |\
    ///                        / +-----------+ \
    ///            /----------/        |        \--------------------\
    ///           /                    |                              \
    ///          v                     v                               v
    /// +----------------+     +-----------------+  +------------------------------------+
    /// | 10, 20, 22, 25 |     | 50, 61, 65, 70  |  | 82, 91, 97, 97, 120, 140, 160, 190 |
    /// +----------------+     +-----------------+  +------------------------------------+
    ///                                                     RMP
    /// ```
    /// ### After the split
    /// ```text
    ///                                 +---------------------+
    ///                                /|     40, 80, 120     |\
    ///                               / +--------+-----+------+ \
    ///            /-----------------/           |     |         \----------------------\
    ///           /                   +----------+     +-------+                         \
    ///          v                    |                        |                          v
    /// +----------------+  +-----------------+    +-----------v----------+    +-----------------+
    /// | 10, 20, 22, 25 |  | 50, 61, 65, 70  |    | 82, 91, 97, 97, 120  |    |  140, 160, 190  |   RMP
    /// +----------------+  +-----------------+    +----------------------+    +-----------------+
    /// ```
    /// We set the right page as the RMP and insert the cell containing the left page's data.
    ///
    /// ### Case2: The split page was not the RMP
    /// The tree before:
    /// ```text
    ///                                       +-----------+
    ///                                      /|  40, 80   |\
    ///                                     / +-----------+ \
    ///            /-----------------------/        |        \-----------------------\
    ///           /                                 |                                 \
    ///          v                                  v                                  v
    /// +----------------+     +--------------------------------------+     +---------------------+
    /// | 10, 20, 22, 25 |     |  50, 62, 63, 64, 68, 69, 71, 73, 79  |     |   82, 91, 97, 97    | RMP
    /// +----------------+     +--------------------------------------+     +---------------------+
    ///                                       OVERFLOW
    /// ```
    /// The pointer to the overflowing page is at index 1 (`80`).
    /// After the split, we need to replace that pointer with the left divider
    /// and insert the right bound as a new key.
    /// ```text
    ///                                       +-----------+
    ///                                      /|40, 68, 79 |\
    ///                                     / +---+----+--+ \
    ///            /-----------------------/      |    |     \----------------------\
    ///           /                   +-----------+    +-----+                       \
    ///          v                    |                      |                        v
    /// +----------------+  +-------------------+   +--------v---------+   +---------------------+
    /// | 10, 20, 22, 25 |  |50, 62, 63, 64, 68 |   |  69, 71, 73, 79  |   |   82, 91, 97, 97    | RMP
    /// +----------------+  +-------------------+   +------------------+   +---------------------+
    /// ```
    /// Everything is balanced:
    ///
    ///
    fn split_then_place<K: LeafKind>(
        &mut self,
        page_no: PageNo,
        key: &Value,
        cell_bytes: &[u8],
    ) -> InkResult<()> {
        let split = self.split_page::<K>(page_no)?;

        self.cursor.stack.pop();
        let parent = self.cursor.stack.pop();
        let split = match parent {
            None => self.grow_root::<K, K::Parent>(&split)?,
            Some(path) => {
                let parent_no = path.page_no;
                drop(path);
                let (idx, slot) =
                    self.parent_slot::<K::Parent>(parent_no, split.left_page, &split.divider)?;
                self.insert_divider::<K::Parent>(parent_no, idx, slot, &split)?;
                split
            }
        };
        self.place_cell::<K>(&split, key, cell_bytes)?;
        Ok(())
    }

    /// Work out where the divider belongs on the parent page.
    ///
    /// When the split child was the right-most one, the divider needs a new
    /// cell. Otherwise the parent already has a cell pointing at that child, and
    /// that cell is handed back so it can take over one of the halves.
    ///
    /// # Errors
    /// When the cell at the slot does not point at the child that was split,
    /// which would mean the tree no longer agrees with itself.
    fn parent_slot<P: InteriorOps>(
        &mut self,
        parent_no: PageNo,
        child: PageNo,
        divider: &Divider,
    ) -> InkResult<(CellIndex, ParentSlot)> {
        let key = divider.key();
        let guard = self.pager.get(parent_no)?;
        let page = parse_ref::<P, V>(parent_no, &guard, self.pager)?;
        let idx = P::slot_for(&page, self.pager, &key)?;

        if page.right_most_ptr()? == Some(child) {
            return Ok((idx, ParentSlot::RightMost));
        }

        let cell = page.cell_bytes_as_ref(idx)?.to_vec();
        let points_at = P::child_of(&page, idx)?;
        if points_at != child {
            return Err(InkError::Corrupt(CorruptError::ParentSlotMismatch {
                parent: parent_no,
                slot: idx,
                points_at,
                expected: child,
            }));
        }
        let key = P::key_of(&page, idx, self.pager)?;
        Ok((
            idx,
            ParentSlot::Existing {
                cell: cell.into(),
                key,
            },
        ))
    }
}

impl<'a, V: Vfs> BTree<'a, V> {
    /// Move the tail of a cell payload onto overflow pages when the cell is too
    /// long for the page it is going into.
    ///
    /// The local part the page keeps follows the SQLite rule, which depends on
    /// whether the cell is going into a table page or an index page. The rest
    /// goes into a chain of fresh pages, each one starting with the number of
    /// the next, and the number of the first is written at the end of the cell.
    pub fn fix_overlow(&mut self, cell: &mut Vec<u8>) -> InkResult<()> {
        if cell.len() <= self.pager.usable_size() {
            return Ok(());
        }
        let usable_size = self.pager.usable_size();
        let (page_no, _) = self.cursor.last_visited_entry_unchecked();
        let index_page = {
            let guard = self.pager.get(page_no)?;
            BTreePage::new(
                page_no,
                self.pager.page_size(),
                usable_size,
                self.pager.header_len(),
                guard.bytes(),
            )?
            .is_index()?
        };
        let local_payload_len = if index_page {
            compute_index_local_payload_size(usable_size, cell.len())
        } else {
            compute_table_local_payload_size(usable_size, cell.len())
        };
        let overflow_data = cell.split_off(local_payload_len);
        let first_overflow_page = self.pager.allocate_new_page()?;
        cell.extend_from_slice(&first_overflow_page.to_be_bytes());

        let mut remaining = overflow_data.len();
        let mut cursor = MemCursor::new(&overflow_data);
        let mut curr_page = first_overflow_page;
        while remaining > 0 {
            let mut guard = self.pager.get_mut(curr_page)?;
            let page_bytes = guard.bytes_as_mut_unchecked();
            let bytes_to_write = remaining.min(usable_size - 4);
            let slice = &mut page_bytes[..usable_size];
            cursor.read_next_exact(&mut slice[4..4 + bytes_to_write])?;
            remaining -= bytes_to_write;
            let next = if remaining == 0 {
                0
            } else {
                self.pager.allocate_new_page()?
            };
            slice[0..4].copy_from_slice(&next.to_be_bytes());
            curr_page = next;
        }
        Ok(())
    }
}

/// The error for a page guard that cannot hand out mutable bytes.
pub(crate) fn guard_not_mutable() -> InkError {
    InkError::Internal("btree: page guard is not mutable")
}

fn not_a_leaf(page_no: PageNo) -> InkError {
    InkError::Corrupt(CorruptError::UnexpectedPageKind {
        page: page_no,
        expected: "leaf",
    })
}

impl<'a, V: Vfs> BTree<'a, V> {
    /// Split a page in two, leaving the left half where it was.
    ///
    /// The cells are divided in the middle, and the cell at the split point
    /// becomes the divider between the halves. What this costs the halves is what
    /// `Consumed` reports: an index entry or a table interior cell comes out of
    /// the left half, while a table leaf keeps every row it had.
    ///
    /// Phase 1, Stage 1: Split the current page into two pages and decide which
    /// cell to promote.
    ///
    /// For table B trees, specifically leaf pages, the promoted key is a copy of
    /// the last cell in the left partition. We do not move or remove the original
    /// cell from the leaf because table B tree interior pages store rowids as
    /// routing keys, not as data.
    ///
    /// When splitting an [`IndexLeaf`], [`super::kind::IndexInterior`], or
    /// [`super::kind::TableInterior`], we need to move the promoted entry for the
    /// following reasons:
    ///
    /// For table interior pages, copying the routing key instead of moving it
    /// would leave a duplicate, which will cause problems later during
    /// redistribution.
    ///
    /// Index interior and leaf pages hold actual entries. Copying an entry instead
    /// of moving it would leave a duplicate, which could violate `UNIQUE` indexes
    /// or cause problems when updating or deleting data.
    ///
    /// The RMP of the new left page, when splitting an interior page, is set to
    /// the left child pointer of the promoted cell.
    /// The RMP of the new right page remains the same as the original RMP.
    ///
    /// # Example
    /// Simple example of a table btree leaf page
    /// ```text
    ///                                           +-----------+
    ///                                          /|  40, 80   |\
    ///                                         / +-----------+ \
    ///                          /-------------/        |        \---------\
    ///                         /                       |                   \
    ///            Overflow    v                        v                    v
    /// +----------------------------------+    +-----------------+  +---------------+
    /// | 10, 20, 22, 25, 29 30, 33, 39 40 |    | 50, 61, 65, 70  |  |82, 91, 97, 97 |
    /// +----------------------------------+    +-----------------+  +---------------+
    ///                                                                      RMP
    /// ```
    /// ### After the split
    /// ```text
    ///                             +-----------+
    ///                            /|  40, 80   |\
    ///                           / +-----------+ \
    ///            /-------------/        |        \---------\
    ///           /                       |                   \
    ///          v                        v                    v
    /// +----------------+        +-----------------+  +---------------+
    /// | 10, 20, 22, 25 |        | 50, 61, 65, 70  |  |82, 91, 97, 97 |
    /// +----------------+        +-----------------+  +---------------+
    ///                                                        RMP
    ///
    ///
    ///    Leaked page +-----------------+
    ///     for now.   |29 30, 33, 39 40 |
    ///                +-----------------+
    /// ```
    /// This function does not link the right page to the parent. That is handled
    /// in Phase 2 by [`BTree::split_then_place`].
    ///
    /// The cell to be promoted has the key [`25`]. Since this is a leaf page, the
    /// parent takes a copy of the cell.
    ///
    /// The RMP is `None` for leaf pages, so nothing needs to be set here. However,
    /// if this were an interior page, the RMP of the new left page would be the
    /// left child of key [`25`], and the key would be moved to the parent.
    ///
    ///
    fn split_page<K: CellOps>(&mut self, page_no: PageNo) -> InkResult<Split> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();

        let (cells, cells_len, split_at, promo, old_rmp) = {
            let guard = self.pager.get(page_no)?;
            let page = parse_ref::<K, V>(page_no, &guard, self.pager)?;
            let n = page.no_of_cells()?;
            if n < 2 {
                return Err(InkError::InternalFmt(format!(
                    "split: page {page_no} holds {n} cell(s)"
                )));
            }
            let split_at = (n / 2) as usize;
            let promo = K::promote(&page, split_at as u16, self.pager)?;
            let mut cells = Vec::new();
            let mut cells_len = Vec::new();
            for i in 0..n {
                let cell = page.cell_bytes_as_ref(i)?;
                cells.extend_from_slice(cell);
                cells_len.push(cell.len());
            }
            (cells, cells_len, split_at, promo, page.right_most_ptr()?)
        };

        if promo.consumed == Consumed::LastOfLeft && split_at < 2 {
            return Err(InkError::InternalFmt(format!(
                "split: page {page_no} has too few cells to promote one"
            )));
        }
        let (left_cells, right_cells, right_start): (&[usize], &[usize], usize) = match promo
            .consumed
        {
            Consumed::None => (&cells_len[..split_at], &cells_len[split_at..], split_at),
            Consumed::LastOfLeft => (&cells_len[..split_at - 1], &cells_len[split_at..], split_at),
            Consumed::FirstOfRight => (
                &cells_len[..split_at],
                &cells_len[split_at + 1..],
                split_at + 1,
            ),
        };

        /*
         write: the right half goes to a fresh page, the left half stays where it was
        */
        let right_page = self.pager.allocate_new_page()?;
        {
            let mut guard = self.pager.get_mut(right_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page =
                TypedPage::<&mut [u8], K>::fresh(right_page, page_size, usable, header_len, bytes)?;
            let mut start: usize = cells_len[..right_start].iter().sum();
            for (i, len) in right_cells.iter().enumerate() {
                let bytes = &cells[start..len + start];
                if page.insert_cell(&bytes, i as _)? == InsertionState::None {
                    return Err(InkError::InternalFmt(format!(
                        "split: right half of page {page_no} does not fit in page {right_page}"
                    )));
                }
                start += len;
            }
            if let Some(rmp) = old_rmp {
                page.set_right_most_ptr(rmp)?;
            }
        };
        {
            let mut guard = self.pager.get_mut(page_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = TypedPage::<&mut [u8], K>::parse_mut(
                page_no, page_size, usable, header_len, bytes,
            )?;
            let mut start = 0;
            page.reset_for_rebuild()?;
            for (i, len) in left_cells.iter().enumerate() {
                let bytes = &cells[start..len + start];
                if page.insert_cell(&bytes, i as u16)? == InsertionState::None {
                    return Err(InkError::InternalFmt(format!(
                        "split: left half of page {page_no} does not fit"
                    )));
                }
                start += len;
            }
            if let Some(rmp) = promo.left_rmp {
                page.set_right_most_ptr(rmp)?;
            }
        }

        let right_bound = {
            let guard = self.pager.get(right_page)?;
            let page = parse_ref::<K, V>(right_page, &guard, self.pager)?;
            let last = page.no_of_cells()?.saturating_sub(1);
            K::divider_of_cell(&page, last, self.pager)?
        };

        Ok(Split {
            left_page: page_no,
            right_page,
            divider: promo.divider,
            right_bound,
        })
    }

    /// Split the root by giving the left half a fresh page of its own and
    /// leaving the root as the new interior page above both halves.
    /// # Example
    /// ```text
    ///                   PageNo (2)
    ///                   Single leaf
    /// +---------------------------------------------------+
    /// |   82, 91, 97, 97, 120, 140, 160, 190, 200, 220    |
    /// +---------------------------------------------------+
    /// ```
    /// After the grow
    /// ```text
    ///                              Root
    ///                            PageNo(2)
    ///                         +------------+
    ///                        /|    120     |\
    ///                       / +------------+ \
    ///   PageNo(4)   /------/                  \--------\
    ///              v                                    v       PageNo(3)
    /// +-------------------------+         +---------------------------+
    /// |   82, 91, 97, 97, 120   |         |  140, 160, 190, 200, 220  |  RMP
    /// +-------------------------+         +---------------------------+
    /// ```
    fn grow_root<K: CellOps, R: InteriorOps>(&mut self, split: &Split) -> InkResult<Split> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();
        let old_root = split.left_page;

        let new_left = self.pager.allocate_new_page()?;
        {
            let mut guard = self.pager.get_mut(new_left)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page =
                TypedPage::<&mut [u8], K>::fresh(new_left, page_size, usable, header_len, bytes)?;
            let old_left_guard = self.pager.get(old_root)?;
            let old_left_page = parse_ref::<K, V>(old_root, &old_left_guard, self.pager)?;
            for i in 0..old_left_page.no_of_cells()? {
                let cell = old_left_page.cell_bytes_as_ref(i as _)?;
                if page.insert_cell(&cell, i as CellIndex)? == InsertionState::None {
                    return Err(InkError::InternalFmt(format!(
                        "grow_root: left half does not fit in page {new_left}"
                    )));
                }
            }
            if let Some(rmp) = old_left_page.right_most_ptr()? {
                page.set_right_most_ptr(rmp)?;
            }
        }

        {
            let divider_cell = split.divider.cell_for(new_left);
            let mut guard = self.pager.get_mut(old_root)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page =
                TypedPage::<&mut [u8], R>::fresh(old_root, page_size, usable, header_len, bytes)?;
            if page.insert_cell(&divider_cell, 0)? == InsertionState::None {
                return Err(InkError::Internal(
                    "grow_root: divider does not fit in a fresh root",
                ));
            }
            page.set_right_most_ptr(split.right_page)?;
        }

        Ok(Split {
            left_page: new_left,
            right_page: split.right_page,
            divider: split.divider.clone(),
            right_bound: split.right_bound.clone(),
        })
    }
}

impl<'a, V: Vfs> BTree<'a, V> {
    /// Put a new divider into a parent page, moving the existing cell when the
    /// split child was the right-most one and adding a cell when it was not.
    ///
    /// When the parent is full the divider cells are held back, the parent is
    /// split, and they are placed into the halves afterward. Holding them rather
    /// than walking the stack again is what lets this go up one page at a time.
    fn insert_divider<P: InteriorOps>(
        &mut self,
        parent_no: PageNo,
        idx: CellIndex,
        slot: ParentSlot,
        split: &Split,
    ) -> InkResult<()> {
        enum Plan {
            MoveHeader,
            KeepOld {
                right_cell: Vec<u8>,
                right_key: Value<'static>,
            },
        }

        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();
        let left_cell = split.divider.cell_for(split.left_page);
        let left_key = split.divider.key();

        let plan = match &slot {
            ParentSlot::RightMost => Plan::MoveHeader,
            ParentSlot::Existing { cell, key } => {
                let right = P::right_divider(cell, key.clone(), split.right_bound.clone());
                Plan::KeepOld {
                    right_cell: right.cell_for(split.right_page),
                    right_key: right.key(),
                }
            }
        };

        let mut pending: Vec<(Value<'static>, Vec<u8>)> = Vec::new();
        {
            let mut guard = self.pager.get_mut(parent_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = TypedPage::<&mut [u8], P>::parse_mut(
                parent_no, page_size, usable, header_len, bytes,
            )?;
            match &plan {
                Plan::MoveHeader => {
                    if page.insert_cell(&left_cell, idx)? == InsertionState::Inserted {
                        page.set_right_most_ptr(split.right_page)?;
                        return Ok(());
                    }
                    pending.push((left_key.clone(), left_cell.clone()));
                }
                Plan::KeepOld {
                    right_cell,
                    right_key,
                } => {
                    page.remove_cell(idx)?;
                    if page.insert_cell(&left_cell, idx)? == InsertionState::Inserted {
                        if page.insert_cell(right_cell, idx + 1)? == InsertionState::Inserted {
                            return Ok(());
                        }
                        pending.push((right_key.clone(), right_cell.clone()));
                    } else {
                        pending.push((left_key.clone(), left_cell.clone()));
                        pending.push((right_key.clone(), right_cell.clone()));
                    }
                }
            }
        }

        let parent_split = self.split_page::<P>(parent_no)?;
        let grandparent = self.cursor.stack.pop();
        match grandparent {
            None => {
                let grown = self.grow_root::<P, P>(&parent_split)?;
                for (key, bytes) in pending {
                    self.place_cell::<P>(&grown, &key, &bytes)?;
                }
            }
            Some(path) => {
                let gp_no = path.page_no;
                drop(path); // release the guard
                let (gp_idx, gp_slot) =
                    self.parent_slot::<P>(gp_no, parent_split.left_page, &parent_split.divider)?;
                self.insert_divider::<P>(gp_no, gp_idx, gp_slot, &parent_split)?;
                for (key, bytes) in pending {
                    self.place_cell::<P>(&parent_split, &key, &bytes)?;
                }
            }
        }

        // We must not forget to set the rightmost pointer.
        if matches!(&plan, Plan::MoveHeader) {
            let mut guard = self.pager.get_mut(parent_split.right_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = TypedPage::<&mut [u8], P>::parse_mut(
                parent_split.right_page,
                page_size,
                usable,
                header_len,
                bytes,
            )?;
            page.set_right_most_ptr(split.right_page)?;
        }
        Ok(())
    }

    /// Insert a cell into the correct half of a split page.
    ///
    /// This is critical because we may need to split the parent when inserting
    /// the new cells, since the parent itself may overflow. When the parent splits,
    /// we lose track of which resulting page should receive the cell. To solve
    /// this, we keep copies of the keys from both the left and right pages, allowing
    /// us to find the correct parent page again and insert the cell into it.
    ///
    /// # Errors
    /// When the cell does not fit in that half, which would mean the split did
    /// not leave enough room for the very cell that caused it.
    fn place_cell<K: CellOps>(
        &mut self,
        split: &Split,
        key: &Value,
        cell_bytes: &[u8],
    ) -> InkResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();
        let target = if *key <= split.divider.key() {
            split.left_page
        } else {
            split.right_page
        };

        let mut guard = self.pager.get_mut(target)?;
        let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
        let mut page =
            TypedPage::<&mut [u8], K>::parse_mut(target, page_size, usable, header_len, bytes)?;
        let idx = K::slot_for(&page, self.pager, key)?;
        if page.insert_cell(&cell_bytes, idx)? == InsertionState::None {
            return Err(InkError::InternalFmt(format!(
                "split: cell does not fit in page {target} right after its split"
            )));
        }
        Ok(())
    }
}

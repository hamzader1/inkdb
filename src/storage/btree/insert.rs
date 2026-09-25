use crate::SqliteResult;
use crate::errors::{CorruptError, SqliteError};
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::btree::CellIndex;
use crate::storage::page::{BTreePage, InsertionState};
use crate::vfs::Vfs;

use super::cursor::BTreeCursor;
use super::kind::{AnyPage, IndexLeaf, TableLeaf, TypedPage};
use super::ops::{CellOps, Consumed, Divider, InteriorOps, LeafKind, ParentSlot, Split};
use super::typed_mut::{AnyPageMut, parse_ref};

pub struct BTree<'a, V: Vfs> {
    pub root_page: PageNo,
    pub pager: &'a mut Pager<V>,
    pub cursor: BTreeCursor<V>,
}

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
    pub fn insert_value(&mut self, key: &Value, cell_bytes: &[u8]) -> SqliteResult<()> {
        self.cursor.seek(self.pager, key)?;
        let (page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();

        let fits = {
            let mut guard = self.pager.get_mut(page_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let page_size = self.pager.page_size();
            let usable = self.pager.usable_size();
            let mut page = AnyPageMut::parse(page_no, page_size, usable, bytes)?;
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
        let guard = self.pager.get(page_no)?;
        match AnyPage::parse(page_no, page_size, usable, guard.bytes())? {
            AnyPage::TableLeaf(_) => self.split_then_place::<TableLeaf>(page_no, key, cell_bytes),
            AnyPage::IndexLeaf(_) => self.split_then_place::<IndexLeaf>(page_no, key, cell_bytes),
            _ => Err(not_a_leaf(page_no)),
        }
    }

    /* Debug aid: the root's children must all be the same kind. Prints the layout and stops at
     * the first insert that breaks it.
     * todo: remove this

    fn debug_check_root_children(&mut self, touched: PageNo, when: &str) {
        if std::env::var("INKDB_CHECK_ROOT").is_err() {
            return;
        }
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let root = self.root_page;
        let Ok(guard) = self.pager.get(root) else {
            return;
        };
        let Ok(page) = BTreePage::<&[u8]>::new(root, page_size, usable, guard.bytes()) else {
            return;
        };
        let Ok(t) = page.page_type() else { return };
        if t.is_leaf() {
            return;
        }
        let n = page.no_of_cells().unwrap_or(0);
        let mut kinds: Vec<(u16, PageNo, u8)> = Vec::new();
        for i in 0..n {
            let Ok(cell) = page.cell(i) else { continue };
            let child = match &cell {
                crate::storage::cell::BTreeCell::IndexInterior(c) => c.left_child,
                crate::storage::cell::BTreeCell::TableInterior(c) => c.left_child,
                _ => continue,
            };
            let k = {
                let g = self.pager.get(child).ok();
                let t = g.as_ref().and_then(|g| {
                    BTreePage::<&[u8]>::new(child, page_size, usable, g.bytes())
                        .ok()
                        .and_then(|p| p.page_type().ok())
                });
                t.map(|t| t as u8).unwrap_or(0)
            };
            kinds.push((i, child, k));
        }
        if let Ok(Some(r)) = page.right_most_ptr() {
            let k = {
                let g = self.pager.get(r).ok();
                let t = g.as_ref().and_then(|g| {
                    BTreePage::<&[u8]>::new(r, page_size, usable, g.bytes())
                        .ok()
                        .and_then(|p| p.page_type().ok())
                });
                t.map(|t| t as u8).unwrap_or(0)
            };
            kinds.push((n, r, k));
        }
        let first_kind = kinds.first().map(|x| x.2).unwrap_or(0);
        if kinds.iter().any(|x| x.2 != first_kind) {
            eprintln!(
                "ROOT MIX after insert into {touched} ({when}), root {root} type {:?}",
                t
            );
            for (slot, child, k) in kinds.iter() {
                eprintln!("   slot {slot} -> page {child} kind {k:#x}");
            }
            panic!("root children mixed after {when}");
        }
    }
    */

    fn split_then_place<K: LeafKind>(
        &mut self,
        page_no: PageNo,
        key: &Value,
        cell_bytes: &[u8],
    ) -> SqliteResult<()> {
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

    fn parent_slot<P: InteriorOps>(
        &mut self,
        parent_no: PageNo,
        child: PageNo,
        divider: &Divider,
    ) -> SqliteResult<(CellIndex, ParentSlot)> {
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
            return Err(SqliteError::Corrupt(CorruptError::ParentSlotMismatch {
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

pub(crate) fn guard_not_mutable() -> SqliteError {
    SqliteError::Internal("btree: page guard is not mutable")
}

fn not_a_leaf(page_no: PageNo) -> SqliteError {
    SqliteError::Corrupt(CorruptError::UnexpectedPageKind {
        page: page_no,
        expected: "leaf",
    })
}

impl<'a, V: Vfs> BTree<'a, V> {
    fn split_page<K: CellOps>(&mut self, page_no: PageNo) -> SqliteResult<Split> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();

        let (cells, cells_len, split_at, promo, old_rmp) = {
            let guard = self.pager.get(page_no)?;
            let page = parse_ref::<K, V>(page_no, &guard, self.pager)?;
            let n = page.no_of_cells()?;
            if n < 2 {
                return Err(SqliteError::InternalFmt(format!(
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
                // cells.push(page.cell_bytes_as_ref(i)?.to_vec());
            }
            (cells, cells_len, split_at, promo, page.right_most_ptr()?)
        };

        if promo.consumed == Consumed::LastOfLeft && split_at < 2 {
            return Err(SqliteError::InternalFmt(format!(
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
        // let right_start = match promo.consumed {
        //     Consumed::None => split_at,
        //     Consumed::LastOfLeft => split_at,
        //     Consumed::FirstOfRight => split_at + 1,
        // };

        /*
         write: the right half goes to a fresh page, the left half stays where it was
        */
        let right_page = self.pager.allocate_new_page()?;
        {
            let mut guard = self.pager.get_mut(right_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = TypedPage::<&mut [u8], K>::fresh(right_page, page_size, usable, bytes)?;
            let mut start: usize = cells_len[..right_start].iter().sum();
            for (i, len) in right_cells.iter().enumerate() {
                let bytes = &cells[start..len + start];
                if page.insert_cell(&bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::InternalFmt(format!(
                        "split: right half of page {page_no} does not fit in page {right_page}"
                    )));
                }
                start += len;
            }
            // for (i, cell) in right_cells.iter().enumerate() {
            //     if page.insert_cell(cell, i as u16)? == InsertionState::None {
            //         return Err(SqliteError::InternalFmt(format!(
            //             "split: right half of page {page_no} does not fit in page {right_page}"
            //         )));
            //     }
            // }
            if let Some(rmp) = old_rmp {
                page.set_right_most_ptr(rmp)?;
            }
        };
        {
            let mut guard = self.pager.get_mut(page_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = TypedPage::<&mut [u8], K>::parse_mut(page_no, page_size, usable, bytes)?;
            let mut start = 0;
            page.reset_for_rebuild()?;
            for (i, len) in left_cells.iter().enumerate() {
                let bytes = &cells[start..len + start];
                if page.insert_cell(&bytes, i as u16)? == InsertionState::None {
                    return Err(SqliteError::InternalFmt(format!(
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

    fn grow_root<K: CellOps, R: InteriorOps>(&mut self, split: &Split) -> SqliteResult<Split> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let old_root = split.left_page;

        /*
         * This hurts performance
         * read: whatever the left half currently holds
         *
        let (cells, rmp) = {
            let guard = self.pager.get(old_root)?;
            let page = parse_ref::<K, V>(old_root, &guard, self.pager)?;
            let n = page.no_of_cells()?;
            let mut cells: Vec<Vec<u8>> = Vec::with_capacity(n as usize);
            for i in 0..n {
                cells.push(page.cell_bytes_as_ref(i)?.to_vec());
            }
            (cells, page.right_most_ptr()?)
        };

        */

        let new_left = self.pager.allocate_new_page()?;
        {
            let mut guard = self.pager.get_mut(new_left)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = TypedPage::<&mut [u8], K>::fresh(new_left, page_size, usable, bytes)?;
            let old_left_guard = self.pager.get(old_root)?;
            let old_left_page = parse_ref::<K, V>(old_root, &old_left_guard, self.pager)?;
            for i in 0..old_left_page.no_of_cells()? {
                let cell = old_left_page.cell_bytes_as_ref(i as _)?;
                if page.insert_cell(&cell, i as CellIndex)? == InsertionState::None {
                    return Err(SqliteError::InternalFmt(format!(
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
            let mut page = TypedPage::<&mut [u8], R>::fresh(old_root, page_size, usable, bytes)?;
            if page.insert_cell(&divider_cell, 0)? == InsertionState::None {
                return Err(SqliteError::Internal(
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
    fn insert_divider<P: InteriorOps>(
        &mut self,
        parent_no: PageNo,
        idx: CellIndex,
        slot: ParentSlot,
        split: &Split,
    ) -> SqliteResult<()> {
        enum Plan {
            MoveHeader,
            KeepOld {
                right_cell: Vec<u8>,
                right_key: Value<'static>,
            },
        }

        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
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
            let mut page =
                TypedPage::<&mut [u8], P>::parse_mut(parent_no, page_size, usable, bytes)?;
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
                drop(path);
                let (gp_idx, gp_slot) =
                    self.parent_slot::<P>(gp_no, parent_split.left_page, &parent_split.divider)?;
                self.insert_divider::<P>(gp_no, gp_idx, gp_slot, &parent_split)?;
                for (key, bytes) in pending {
                    self.place_cell::<P>(&parent_split, &key, &bytes)?;
                }
            }
        }
        if matches!(&plan, Plan::MoveHeader) {
            let mut guard = self.pager.get_mut(parent_split.right_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = TypedPage::<&mut [u8], P>::parse_mut(
                parent_split.right_page,
                page_size,
                usable,
                bytes,
            )?;
            page.set_right_most_ptr(split.right_page)?;
        }
        Ok(())
    }

    fn place_cell<K: CellOps>(
        &mut self,
        split: &Split,
        key: &Value,
        cell_bytes: &[u8],
    ) -> SqliteResult<()> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let target = if *key <= split.divider.key() {
            split.left_page
        } else {
            split.right_page
        };

        let mut guard = self.pager.get_mut(target)?;
        let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
        let mut page = TypedPage::<&mut [u8], K>::parse_mut(target, page_size, usable, bytes)?;
        let idx = K::slot_for(&page, self.pager, key)?;
        if page.insert_cell(&cell_bytes, idx)? == InsertionState::None {
            return Err(SqliteError::InternalFmt(format!(
                "split: cell does not fit in page {target} right after its split"
            )));
        }
        Ok(())
    }
}

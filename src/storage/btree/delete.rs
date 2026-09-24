use crate::SqliteError;
use crate::SqliteResult;
use crate::pager::pager::PageNo;
use crate::record::Value;
use crate::storage::page::{BTreePage, PageMut, PageRef};
use crate::vfs::Vfs;

use super::insert::{BTree, guard_not_mutable};
use super::kind::AnyPage;
use crate::storage::btree::CellIndex;

impl<'a, V: Vfs> BTree<'a, V> {
    pub fn delete_value(&mut self, key: &Value) -> SqliteResult<bool> {
        let _ = self.cursor.seek_for_delete(self.pager, key)?;
        let Some((page_no, cell_idx)) = self.cursor.last_visited_entry() else {
            return Ok(false);
        };
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();

        let (is_leaf, is_index, n, found) = {
            let guard = self.pager.get(page_no)?;
            let any = AnyPage::parse(page_no, page_size, usable, guard.bytes())?;
            let is_leaf = matches!(any, AnyPage::TableLeaf(_) | AnyPage::IndexLeaf(_));
            let is_index = matches!(any, AnyPage::IndexLeaf(_) | AnyPage::IndexInterior(_));
            let n = any.no_of_cells()?;
            let found = if cell_idx < n {
                Some(any.cell_key(cell_idx, self.pager)?)
            } else {
                None
            };
            (is_leaf, is_index, n, found)
        };
        if cell_idx >= n {
            return Ok(false);
        }
        if found.as_ref() != Some(key) {
            return Ok(false);
        }

        if is_leaf {
            let underflow = {
                let mut guard = self.pager.get_mut(page_no)?;
                let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
                let mut page = PageMut::new(page_no, page_size, usable, bytes)?;
                page.remove_cell(cell_idx)?;
                page.is_underflow()?
            };
            if underflow && page_no != self.root_page {
                self.fix_page_underflow(page_no)?;
            }
            return Ok(true);
        }

        if !is_index {
            return Ok(false);
        }
        self.delete_index_divider(page_no, cell_idx)
    }

    fn delete_index_divider(&mut self, page_no: PageNo, cell_idx: CellIndex) -> SqliteResult<bool> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();

        let divider_left_child = {
            let guard = self.pager.get(page_no)?;
            let at = {
                let page = BTreePage::<&[u8]>::new(page_no, page_size, usable, guard.bytes())?;
                page.cell_ptr(cell_idx)? as usize
            };
            let bytes = guard.bytes();
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };

        let mut pred_no = divider_left_child;
        loop {
            let guard = self.pager.get(pred_no)?;
            let any = AnyPage::parse(pred_no, page_size, usable, guard.bytes())?;
            let n = any.no_of_cells()?;
            if matches!(any, AnyPage::TableLeaf(_) | AnyPage::IndexLeaf(_)) {
                if n == 0 {
                    return Err(SqliteError::Corrupt(
                        "index predecessor leaf is empty".into(),
                    ));
                }
                self.cursor
                    .stack
                    .push(super::cursor::Path::new(pred_no, n - 1, guard));
                break;
            }
            let rmp = PageRef::new(pred_no, page_size, usable, guard.bytes())?
                .right_most_ptr()?
                .ok_or_else(|| {
                    SqliteError::Corrupt("index interior has no right-most child".into())
                })?;
            self.cursor
                .stack
                .push(super::cursor::Path::new(pred_no, n, guard));
            pred_no = rmp;
        }

        let (pred_page, pred_idx) = self.cursor.last_visited_entry_unchecked();
        let pred_bytes = {
            let guard = self.pager.get(pred_page)?;
            let page = BTreePage::<&[u8]>::new(pred_page, page_size, usable, guard.bytes())?;
            page.cell_bytes_as_ref(pred_idx)?.to_vec()
        };
        let new_divider = crate::storage::cell::Encode::encode_index_interior_cell(
            divider_left_child,
            &pred_bytes,
        );
        {
            let mut guard = self.pager.get_mut(page_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = PageMut::new(page_no, page_size, usable, bytes)?;
            if page.replace_cell(cell_idx, &new_divider)?
                == crate::storage::page::InsertionState::None
            {
                return Err(SqliteError::Internal(
                    "index divider repaint does not fit in its parent".into(),
                ));
            }
        }

        let underflow = {
            let mut guard = self.pager.get_mut(pred_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = PageMut::new(pred_page, page_size, usable, bytes)?;
            page.remove_cell(pred_idx)?;
            page.is_underflow()?
        };
        if underflow && pred_page != self.root_page {
            self.fix_page_underflow(pred_page)?;
        }
        Ok(true)
    }
}

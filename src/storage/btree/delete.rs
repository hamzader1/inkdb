use crate::InkError;
use crate::InkResult;
use crate::errors::CorruptError;
use crate::pager::pager::PageNo;
use crate::record::Value;
use crate::storage::cell::Encode;
use crate::storage::page::InsertionState;
use crate::storage::page::{BTreePage, PageMut, PageRef};
use crate::vfs::Vfs;

use super::cursor::Path;
use super::insert::guard_not_mutable;
use super::kind::AnyPage;
use super::tree::BTree;
use crate::storage::btree::CellIndex;

impl<'a, V: Vfs> BTree<'a, V> {
    /// Delete the entry at the cursor's position (after the seek). The cursor may point to
    /// an arbitrary location after the deletion.
    ///
    /// A leaf cell is dropped straight away. A key that sits on an interior page
    /// of an index tree cannot simply go, since it is the divider between two
    /// children, so it is handled separately.
    pub fn delete_value(&mut self, key: &Value) -> InkResult<bool> {
        let _ = self.cursor.seek_for_delete(self.pager, key)?;
        let Some((page_no, cell_idx)) = self.cursor.last_visited_entry() else {
            return Ok(false);
        };
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();

        let (is_leaf, is_index, n, found) = {
            let guard = self.pager.get(page_no)?;
            let any = AnyPage::parse(page_no, page_size, usable, header_len, guard.bytes())?;
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
                let mut page = PageMut::new(page_no, page_size, usable, header_len, bytes)?;
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

    /// Remove an index divider by moving its predecessor entry into its place.
    ///
    /// The divider has to stay, because the children on either side of it still
    /// need separating, so the entry just before it is taken out of the last leaf
    /// of the divider left child and written into the parent cell. That leaf is
    /// reached by following right-most pointers, and it is not allowed to be
    /// empty.
    /// # Example
    /// Let's walk this example
    /// ```text
    ///                                            +--------------+
    ///                                            |Non underflow |
    ///                                           /|     root     |
    ///                                          / +--------------+
    ///                               /---------/
    ///                              v
    ///                          PageNo(3)
    ///                     +----+------+-----+
    ///                    /| P0 |David | 42  |
    ///                   / +----+------+-----+\               Entry we
    ///                  /  | P1 | leam | 82  |<-------------- want to
    ///        /--------/   +--+-+------+-----+  \             delete
    ///       /                |                  \---\
    ///      /                 +---+                   \
    ///     v                      |                    \
    /// +------+-----+             v PageNo(7)           v
    /// |Alice | 10  |         +------+-----+        +------+-----+
    /// +------+-----+         | Eve  | 51  |        |Linus | 91  |
    /// |Alice | 20  |         +------+-----+        +------+-----+
    /// +------+-----+         |Terry | 60  |        |Terry | 97  |
    /// | Bob  | 30  |         +------+-----+        +------+-----+
    /// +------+-----+         |Davis | 70  |        |Davis | 99  |
    /// |Carol | 40  |         +------+-----+        +------+-----+
    /// +------+-----+
    ///
    ///```
    /// This function (`delete_index_divider`) receives (`page_no: 3`, `cell_idx: 1`).
    /// Now we need to get the divider's left child page number. For the entry
    /// ["Liam", 82] in this diagram, it is PageNo(7).
    /// We start the loop, and the first `if` condition is true because the child
    /// page is a leaf. We select its last entry using `no_of_cells - 1`, which
    /// gives us the index of the last cell.
    /// ```text
    ///                                            +--------------+
    ///                                            |Non underflow |
    ///                                           /|     root     |
    ///                                          / +--------------+
    ///                               /---------/
    ///                              v
    ///                     +----+------+-----+
    ///                    /| P0 |David | 42  |
    ///                   / +----+------+-----+\               Entry we
    ///                  /  | P1 | leam | 82  |<-------------- want to
    ///        /--------/   +--+-+------+-----+  \             delete
    ///       /                |                  \---\
    ///      /                 +---+                   \
    ///     v                      |                    \
    /// +------+-----+             v                     v
    /// |Alice | 10  |         +------+-----+        +------+-----+
    /// +------+-----+         | Eve  | 51  |        |Linus | 91  |
    /// |Alice | 20  |         +------+-----+        +------+-----+
    /// +------+-----+         |Terry | 60  |        |Terry | 97  |
    /// | Bob  | 30  |         +------+-----+        +------+-----+
    /// +------+-----+         |Davis | 70  |<-+     |Davis | 99  |
    /// |Carol | 40  |         +------+-----+  |     +------+-----+
    /// +------+-----+                         |
    ///                                        +-- This cell
    /// ```
    /// We take this cell's data and replace the entry we want to delete.
    /// Then we delete the original cell from the child to prevent duplication.
    /// The final tree is:
    /// ```text
    ///                                            +--------------+
    ///                                            |Non underflow |
    ///  New cell----+                            /|     root     |
    ///              |                           / +--------------+
    ///              |                /---------/
    ///              |               v
    ///              |      +----+------+-----+
    ///              |     /| P0 |David | 42  |
    ///              |    / +----+------+-----+\
    ///              +----> | P1 |Davis | 70  | \
    ///        /--------/   +--+-+------+-----+  \
    ///       /                |                  \---\
    ///      /                 +---+                   \
    ///     v                      |                    \
    /// +------+-----+             v                     v
    /// |Alice | 10  |         +------+-----+        +------+-----+
    /// +------+-----+         | Eve  | 51  |        |Linus | 91  |
    /// |Alice | 20  |         +------+-----+        +------+-----+
    /// +------+-----+         |Terry | 60  |        |Terry | 97  |
    /// | Bob  | 30  |         +------+-----+        +------+-----+
    /// +------+-----+                               |Davis | 99  |
    /// |Carol | 40  |                               +------+-----+
    /// +------+-----+
    /// ```
    ///
    fn delete_index_divider(&mut self, page_no: PageNo, cell_idx: CellIndex) -> InkResult<bool> {
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let header_len = self.pager.header_len();

        let divider_left_child = {
            let guard = self.pager.get(page_no)?;
            let at = {
                let page =
                    BTreePage::<&[u8]>::new(page_no, page_size, usable, header_len, guard.bytes())?;
                page.cell_ptr(cell_idx)? as usize
            };
            let bytes = guard.bytes();
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };

        let mut pred_no = divider_left_child;
        loop {
            let guard = self.pager.get(pred_no)?;
            let any = AnyPage::parse(pred_no, page_size, usable, header_len, guard.bytes())?;
            let n = any.no_of_cells()?;
            if matches!(any, AnyPage::TableLeaf(_) | AnyPage::IndexLeaf(_)) {
                if n == 0 {
                    return Err(InkError::Corrupt(CorruptError::EmptyPredecessorLeaf));
                }
                self.cursor.stack.push(Path::new(pred_no, n - 1, guard));
                break;
            }
            let rmp = PageRef::new(pred_no, page_size, usable, header_len, guard.bytes())?
                .right_most_ptr()?
                .ok_or({
                    InkError::Corrupt(CorruptError::MissingRightMostChild { page: pred_no })
                })?;
            self.cursor.stack.push(Path::new(pred_no, n, guard));
            pred_no = rmp;
        }

        let (pred_page, pred_idx) = self.cursor.last_visited_entry_unchecked();
        let pred_bytes = {
            let guard = self.pager.get(pred_page)?;
            let page =
                BTreePage::<&[u8]>::new(pred_page, page_size, usable, header_len, guard.bytes())?;
            page.cell_bytes_as_ref(pred_idx)?.to_vec()
        };
        let new_divider = Encode::encode_index_interior_cell(divider_left_child, &pred_bytes);
        {
            let mut guard = self.pager.get_mut(page_no)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = PageMut::new(page_no, page_size, usable, header_len, bytes)?;
            if page.replace_cell(cell_idx, &new_divider)? == InsertionState::None {
                return Err(InkError::Internal(
                    "index divider repaint does not fit in its parent",
                ));
            }
        }

        let underflow = {
            let mut guard = self.pager.get_mut(pred_page)?;
            let bytes = guard.bytes_as_mut().ok_or_else(guard_not_mutable)?;
            let mut page = PageMut::new(pred_page, page_size, usable, header_len, bytes)?;
            page.remove_cell(pred_idx)?;
            page.is_underflow()?
        };
        if underflow && pred_page != self.root_page {
            self.fix_page_underflow(pred_page)?;
        }
        Ok(true)
    }
}

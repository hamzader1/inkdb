use super::kind::HasChild;
use crate::InkResult;
use crate::errors::InkError;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::record::tuple::Tuple;
use crate::storage::btree::{CellIndex, compare_index_entry, page_as_ref_with_pager};
use crate::storage::page::PageRef;
use crate::vfs::Vfs;
use std::cmp::Ordering;

use self::IndexSearchResult::{EqualPrefix, NotFound};

use super::kind::{
    AnyPage, Cell, HasPayload, HasRowId, IndexInterior, IndexKind, PageKind, TableInterior,
    TypedPage,
};

/// Where a cursor is, as far as the tree is concerned.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum CursorState {
    /// On a cell.
    At,
    /// Nothing has been asked of it yet.
    Invalid,
    /// Past the largest key in the tree.
    AfterLast,
    /// Past the smallest key in the tree.
    BeforeFirst,
}

/// What a seek found.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SeekResult {
    /// The key is in the tree, and the cursor is on it.
    Exact,
    /// The key is not in the tree. The cursor sits on the cell just after where
    /// it would have gone, which is where it would be stored.
    NotFound,
}

/// One page on the way down to the cursor, and the cell it stopped on.
///
/// The whole stack of paths is kept, so walking back and forth is a matter of
/// looking at the last one. Each path holds its page guard, which is what keeps
/// the pages on the way down from being evicted while the cursor sits on them.
//  Keep all visited pages in memory (by holding their guard) so a later rebalance is fast.
#[derive(Debug)]
pub(crate) struct Path {
    /// The page.
    pub page_no: PageNo,
    /// The slot on that page the walk stopped at.
    pub cell_idx: u16,
    /// The guard that keeps the page in memory.
    pub(crate) guard: PageGuard,
    /// Whether this path cell has already been given out by the cursor.
    pub yielded: bool,
}
impl Path {
    pub(crate) fn new(page_no: PageNo, cell_idx: CellIndex, guard: PageGuard) -> Self {
        Self {
            page_no,
            cell_idx,
            guard,
            yielded: false,
        }
    }
}

/// What came of finding a saved position again.
/// The very key the cursor was on is still there.
pub(crate) enum RestorePosition {
    Exact,
    /// The key has gone, and the cursor moved on to the cell that took its
    /// place.
    Next,
    /// There was no saved position to go back to.
    Empty,
}

/// Where a key sits on an index page, as found by binary search.
pub(crate) enum IndexSearchResult {
    /// The cell holding exactly this entry.
    Exact(u16),
    /// The first cell whose leading columns match, when the key names fewer
    /// columns than the entry holds.
    EqualPrefix(u16),
    /// The cell just after where the key would have gone.
    NotFound(u16),
}

/// A place in a b-tree, kept as the stack of pages from the root down.
#[derive(Debug)]
pub struct BTreeCursor<V: Vfs> {
    /// The page the tree starts at.
    pub(crate) root: PageNo,
    /// The pages on the way down, root first and the cursor page last.
    pub(crate) stack: Vec<Path>,
    /// Where in the tree the cursor is.
    pub(crate) state: CursorState,
    /// The key of the cell the cursor was on when its position was saved.
    /// We use this to restore the position where the cursor was sitting after
    /// deleting or moving a value.
    pub(crate) saved_key: Option<Value<'static>>,
    /// Whether that saved cell had already been given out.
    saved_yielded: bool,
    _phantom: std::marker::PhantomData<V>,
}

impl<V: crate::vfs::Vfs> BTreeCursor<V> {
    pub fn new(root: PageNo) -> Self {
        Self {
            root,
            stack: Vec::new(),
            state: CursorState::Invalid,
            saved_key: None,
            saved_yielded: false,
            _phantom: std::marker::PhantomData,
        }
    }

    /*
     * todo: optimize these two functions below
     */
    /// Keep the cell the cursor is on as a key and drop the pages.
    ///
    /// A cursor cannot hold pages between two statements, so the key is the only
    /// thing that can be kept. If the cursor is past the cells of its page there
    /// is nothing to keep, and the saved key is cleared.
    pub fn save_position(&mut self, pager: &mut Pager<V>) -> InkResult<()> {
        if let Some(last_entry) = self.stack.last() {
            let guard = pager.get(last_entry.page_no)?;
            let page = AnyPage::parse(
                last_entry.page_no,
                pager.page_size(),
                pager.usable_size(),
                pager.header_len(),
                guard.bytes(),
            )?;
            if last_entry.cell_idx >= page.no_of_cells()? {
                self.saved_key = None;
                self.saved_yielded = false;
                self.stack.clear();
                return Ok(());
            }
            self.saved_yielded = last_entry.yielded;
            let i = last_entry.cell_idx;
            let key = page.cell_key(i, pager)?;
            self.saved_key = Some(key);
            self.stack.clear();
        }
        Ok(())
    }
    /// Seek back to the saved key.
    ///
    /// Keep track of whether the saved cell was already given out, so resuming
    /// the walk does not return the same cell twice.
    pub(crate) fn restore_position(&mut self, pager: &mut Pager<V>) -> InkResult<RestorePosition> {
        // No key was saved
        let Some(key) = self.saved_key.take() else {
            return Ok(RestorePosition::Empty);
        };
        // If the entry was already yielded, this only applies to interior index
        // pages, since they also hold entries.
        let saved_yielded = self.saved_yielded;
        self.saved_yielded = false;
        // seek_internal would stop if the key matched an internal key
        self.seek_internal(pager, &key, true)?;
        let Some(path) = self.stack.last_mut() else {
            return Ok(RestorePosition::Next);
        };
        // If the key was already yielded, restore the flag so we don't
        // visit the same entry twice.
        if saved_yielded {
            let guard = pager.get(path.page_no)?;
            let any = AnyPage::parse(
                path.page_no,
                pager.page_size(),
                pager.usable_size(),
                pager.header_len(),
                guard.bytes(),
            )?;
            if !matches!(any, AnyPage::TableLeaf(_) | AnyPage::IndexLeaf(_)) {
                path.yielded = true;
            }
        }
        let (page_no, cell_idx) = (path.page_no, path.cell_idx);
        let guard = pager.get(page_no)?;
        let page = AnyPage::parse(
            page_no,
            pager.page_size(),
            pager.usable_size(),
            pager.header_len(),
            guard.bytes(),
        )?;
        // If there is at least one entry ahead, check if it matches.
        if cell_idx < page.no_of_cells()? {
            let i = cell_idx;
            let cell_key = page.cell_key(i, pager)?;

            if cell_key == key {
                return Ok(RestorePosition::Exact);
            }
        }
        // If the moved key was the last child, move to the first child
        // of the next sibling.
        /*Use diagram*/
        self.skip_past_end(pager)?;
        Ok(RestorePosition::Next)
    }

    /// Step forward until the cursor is on a cell, or past the end of the tree.
    ///
    /// A seek can leave the cursor on the slot a missing key would go in, which
    /// is not a cell. Stepping once past it lands on the next real entry.
    pub fn skip_past_end(&mut self, pager: &mut Pager<V>) -> InkResult<()> {
        loop {
            let Some(path) = self.stack.last() else {
                self.state = CursorState::AfterLast;
                return Ok(());
            };
            let page = page_as_ref_with_pager(path.page_no, &path.guard, pager)?;
            if path.cell_idx < page.no_of_cells()? {
                self.state = CursorState::At;
                return Ok(());
            }
            self.next(pager)?;
            if self.state == CursorState::AfterLast {
                return Ok(());
            }
        }
    }

    /// Move to the first entry that is not less than the key.
    ///
    /// When the key is absent, the cursor ends up on the first entry that is
    /// greater than it.
    pub fn seek_lower_bound(
        &mut self,
        pager: &mut Pager<V>,
        target: &Value<'_>,
    ) -> InkResult<SeekResult> {
        let res = self.seek_internal(pager, target, true)?;
        self.skip_past_end(pager)?;
        Ok(res)
    }

    /// Whether the cursor is sitting on a cell it can give out.
    pub fn is_valid(&self) -> bool {
        self.state == CursorState::At && !self.stack.is_empty()
    }

    /// Move to a key, always ending on a leaf.
    ///
    /// Move the cursor to the entry matching the target key. If no exact match is
    /// found, the cursor stops at the leaf that would contain the key and points
    /// to the first entry after it.
    ///
    /// # How the seek works
    /// Start at the root and follow the child pointer whose key range can contain
    /// the target. For an internal node with keys `Key(0)` through `Key(n - 1)`
    /// and pointers `Ptr(0)` through `Ptr(n)`, follow `Ptr(n)` if the target is
    /// greater than the last key. Otherwise, find the first key greater than or
    /// equal to the target and follow the pointer immediately before it.
    ///
    /// Repeat this until reaching a leaf, then search its entries for the target.
    ///
    pub fn seek(
        &mut self,
        pager: &mut Pager<V>,
        target: &Value<'_>,
    ) -> Result<SeekResult, InkError> {
        self.seek_internal(pager, target, false)
    }
    /// Move to a key, stopping early when it is found on an interior page.
    ///
    /// An index divider is an entry that exists only on the interior page that
    /// holds it, so deleting one has to stop on the way down rather than finish
    /// at a leaf.
    pub fn seek_for_delete(
        &mut self,
        pager: &mut Pager<V>,
        target: &Value<'_>,
    ) -> Result<SeekResult, InkError> {
        self.seek_internal(pager, target, true)
    }
    /// The walk shared by the three seek variants.
    ///
    /// Table pages always descend, since a row only ever lives on a leaf.
    /// Index pages stop on the interior cell when the key matches it exactly and
    /// `stop_at_interior` asks for that, and otherwise follow the child the
    /// search points at.
    fn seek_internal(
        &mut self,
        pager: &mut Pager<V>,
        target: &Value,
        stop_at_interior: bool,
    ) -> InkResult<SeekResult> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            match AnyPage::parse(
                page_no,
                pager.page_size(),
                pager.usable_size(),
                pager.header_len(),
                guard.bytes(),
            )? {
                AnyPage::TableInterior(ref p) => {
                    let (_, i) = search_row_ids(p, pager, target.cast_int()? as _)?;
                    let child = if i < p.no_of_cells()? {
                        p.cell(i)?.left_child()
                    } else {
                        p.rmp()?
                    };
                    self.stack.push(Path::new(page_no, i, guard));
                    self.state = CursorState::At;
                    page_no = child;
                }
                AnyPage::TableLeaf(ref p) => {
                    let (found, i) = search_row_ids(p, pager, target.cast_int()? as _)?;
                    self.stack.push(Path::new(page_no, i, guard));
                    self.state = CursorState::At;
                    if found {
                        return Ok(SeekResult::Exact);
                    }
                    return Ok(SeekResult::NotFound);
                }
                AnyPage::IndexInterior(ref p) => match search_indexes(p, pager, target)? {
                    IndexSearchResult::Exact(i) if stop_at_interior => {
                        self.stack.push(Path::new(page_no, i, guard));
                        self.state = CursorState::At;
                        return Ok(SeekResult::Exact);
                    }
                    IndexSearchResult::Exact(i) | IndexSearchResult::EqualPrefix(i) => {
                        let child = p.cell(i)?.left_child();
                        self.stack.push(Path::new(page_no, i, guard));
                        self.state = CursorState::At;
                        page_no = child;
                    }
                    IndexSearchResult::NotFound(i) => {
                        let child = if i < p.no_of_cells()? {
                            p.cell(i)?.left_child()
                        } else {
                            p.rmp()?
                        };
                        self.stack.push(Path::new(page_no, i, guard));
                        self.state = CursorState::At;
                        page_no = child;
                    }
                },
                AnyPage::IndexLeaf(ref p) => match search_indexes(p, pager, target)? {
                    IndexSearchResult::Exact(i) | IndexSearchResult::EqualPrefix(i) => {
                        self.state = CursorState::At;
                        self.stack.push(Path::new(page_no, i, guard));
                        return Ok(SeekResult::Exact);
                    }
                    IndexSearchResult::NotFound(i) => {
                        self.state = CursorState::At;
                        self.stack.push(Path::new(page_no, i, guard));
                        return Ok(SeekResult::NotFound);
                    }
                },
            }
        }
    }
    /// Step forward one entry.
    ///
    /// This function moves a cursor to the next element after the one it is
    /// currently pointing to.
    ///
    /// To find the next entry, first check whether there is another entry on the
    /// current leaf. If there is, return it.
    ///
    /// Otherwise, move up through the parents until reaching a node that is not
    /// the rightmost child of its parent. Move to the next sibling subtree, then
    /// follow its leftmost children until reaching a leaf. The first entry in that
    /// leaf is the next entry.
    ///
    /// If the current leaf is the rightmost leaf of the tree, there is no next
    /// entry and the cursor reaches the end of the tree.
    /// # Example
    /// This is an example of Table Btree
    /// ``` text
    ///
    ///             Parent
    ///                 |
    ///                 +------+
    ///                        |
    ///                     +--v--------+
    ///                    /|  40, 80   |\
    ///                   / +-----------+ \
    ///         /--------/        |        \---------\
    ///        /                  |                   \
    ///       v                   v                    v
    /// +-----------+     +---------------+    +---------------+
    /// |10, 20, 30 |     |42, 51, 60, 70 |    |82, 91, 97, 97 |
    /// +-----------+     +---------------+    +---------------+
    ///           ^                                    RMP
    ///           |
    ///           |
    ///        Current/
    ///
    /// ```
    /// Once we call `next` and there is no next sibling, move to the first entry
    /// of the next parent.
    ///
    /// ```text
    ///                         Parent
    ///                            |
    ///                            |
    ///                            |
    ///                     +------v----+
    ///                    /|  40, 80   |\
    ///                   / +-----------+ \
    ///         /--------/        |        \---------\
    ///        /                  |                   \
    ///       v                   v                    v
    /// +-----------+     +---------------+    +---------------+
    /// |10, 20, 30 |     |42, 51, 60, 70 |    |82, 91, 97, 97 |
    /// +-----------+     +---------------+    +---------------+
    ///                     ^                          RMP
    ///                     |
    ///                     |
    ///
    ///                  Current
    /// ```
    ///
    /// For an index tree, interior cells are entries themselves, so we stop at the
    /// interior cell and mark it as yielded.
    /// ```text
    ///
    ///                                                            +----+------+-----+
    ///                                                           /| P0 | leam | 101 |\
    ///                                                          / +----+------+-----+ \
    ///                                                         /                       \
    ///                                      /-----------------/                         \---------------------------\
    ///  Pointer after calling              /                                                                         \
    ///           next                     v                                                                           \
    ///                           +----+------+-----+                                                                   v
    ///             |            /| P0 |David | 42  |                                                          +----+------+-----+
    ///             |           / +----+------+-----+\                                                         | P0 |Olivia| 120 |
    ///             +----------X->| P1 | leam | 82  | \                                                        +----+------+-----+\
    ///                       /   +--+-+------+-----+  \                                                      /| P1 |Rupert| 140 | \
    ///              /-------/       |                  \---\                                                / +----+------+-----+  \
    ///             /                +---+                   \                                              /           |            \
    ///            /                     |                    \                                            /            |             \----\
    ///           v                      v                     v                                    /-----/             |                   \
    ///       +------+-----+         +------+-----+        +------+-----+                          /                    |                    \
    ///       |Alice | 10  |         | Eve  | 51  |        |Linus | 91  |                         /                     |                     \
    ///       +------+-----+         +------+-----+        +------+-----+                        v                      |                      v
    ///       |Alice | 20  |         |Terry | 60  |        |Terry | 97  |              +------------------+   +---------v--------+   +------------------+
    ///       +------+-----+         +------+-----+        +------+-----+              |       ...        |   |       ...        |   |       ...        |
    ///    +->| Bob  | 30  |         |Davis | 70  |        |Davis | 99  |              +------------------+   +------------------+   +------------------+
    ///    |  +------+-----+         +------+-----+        +------+-----+
    ///    |
    ///    +-------+
    ///            |
    ///            |
    /// Pointer before calling
    ///          next
    /// ```
    /// In our case:
    /// The cursor is popped from the stack and asked what to do next: stay on the
    /// page, drop it and let the page above continue, or go down into a subtree.
    /// Interior index pages can yield their own cells while being traversed.
    /// Once there is nothing left to visit at the top, the cursor is positioned
    /// past the end of the tree.
    pub fn next(&mut self, pager: &mut Pager<V>) -> InkResult<()> {
        while let Some(path) = self.stack.pop() {
            let Path {
                page_no,
                cell_idx,
                guard,
                yielded,
            } = path;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            let any = AnyPage::parse(
                page_no,
                pager.page_size(),
                pager.usable_size(),
                pager.header_len(),
                page.bytes(),
            )?;
            let max = any.no_of_cells()?;

            let step = match &any {
                AnyPage::TableLeaf(_) | AnyPage::IndexLeaf(_) => leaf_next_step(cell_idx, max),
                AnyPage::IndexInterior(p) => index_interior_next_step(p, cell_idx, max, yielded)?,
                AnyPage::TableInterior(p) => table_interior_next_step(p, cell_idx, max)?,
            };

            match step {
                Step::PopParent => continue,
                Step::Stay { idx, yielded } => {
                    self.stack.push(Path {
                        page_no,
                        cell_idx: idx,
                        guard,
                        yielded,
                    });
                    self.state = CursorState::At;
                    return Ok(());
                }
                Step::Descend {
                    child,
                    push_idx,
                    push_yielded,
                } => {
                    self.stack.push(Path {
                        page_no,
                        cell_idx: push_idx,
                        guard,
                        yielded: push_yielded,
                    });
                    self.descend_to_first(pager, child)?;
                    self.state = CursorState::At;
                    return Ok(());
                }
            }
        }
        self.state = CursorState::AfterLast;
        Ok(())
    }

    /// Walk down the left side of the tree, taking the first cell of every page.
    pub fn descend_to_first(
        &mut self,
        pager: &mut Pager<V>,
        page_no: PageNo,
    ) -> Result<(), InkError> {
        let mut page_no = page_no;
        loop {
            let guard = pager.get(page_no)?;
            let page = AnyPage::parse(
                page_no,
                pager.page_size(),
                pager.usable_size(),
                pager.header_len(),
                guard.bytes(),
            )?;
            page_no = {
                let child = match page {
                    AnyPage::IndexLeaf(_) | AnyPage::TableLeaf(_) => {
                        self.add_path(page_no, 0, guard);
                        return Ok(());
                    }
                    AnyPage::IndexInterior(p) => p.cell(0)?.left_child(),
                    AnyPage::TableInterior(p) => p.cell(0)?.left_child(),
                };
                self.add_path(page_no, 0, guard);
                child
            };
        }
    }
    /// This function moves a cursor to the first element in the tree, that is,
    /// to the left most descendant node of the tree.
    pub fn first(&mut self, pager: &mut Pager<V>) -> Result<(), InkError> {
        self.clear_path();
        let root = self.root;
        self.descend_to_first(pager, root)
    }

    /// Step back one entry.
    ///
    /// Move the cursor to the entry before the one it currently points to.
    ///
    /// This is the reverse of `next`, with the same three possible actions on each
    /// page. An interior index cell is yielded when leaving its subtree rather than
    /// when entering it. Once there is nothing left to visit, the cursor is before
    /// the first entry.
    /// See the diagrams of [`BTreeCursor::next`]
    ///
    pub fn prev(&mut self, pager: &mut Pager<V>) -> Result<(), InkError> {
        while let Some(path) = self.stack.pop() {
            let Path {
                page_no,
                cell_idx,
                guard,
                yielded,
            } = path;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            let any = AnyPage::parse(
                page_no,
                pager.page_size(),
                pager.usable_size(),
                pager.header_len(),
                page.bytes(),
            )?;

            let step = match &any {
                AnyPage::TableLeaf(_) | AnyPage::IndexLeaf(_) => leaf_prev_step(cell_idx),
                AnyPage::IndexInterior(p) => index_interior_prev_step(p, cell_idx, yielded)?,
                AnyPage::TableInterior(p) => table_interior_prev_step(p, cell_idx)?,
            };

            match step {
                Step::PopParent => continue,
                Step::Stay { idx, yielded } => {
                    self.stack.push(Path {
                        page_no,
                        cell_idx: idx,
                        guard,
                        yielded,
                    });
                    self.state = CursorState::At;
                    return Ok(());
                }
                Step::Descend {
                    child,
                    push_idx,
                    push_yielded,
                } => {
                    self.stack.push(Path {
                        page_no,
                        cell_idx: push_idx,
                        guard,
                        yielded: push_yielded,
                    });
                    self.descend_to_last(pager, child)?;
                    self.state = CursorState::At;
                    return Ok(());
                }
            }
        }
        self.state = CursorState::BeforeFirst;
        Ok(())
    }

    /// This function moves a cursor to the last element in the tree, that is, to
    /// the right most descendant node of the tree.
    pub fn last(&mut self, pager: &mut Pager<V>) -> Result<(), InkError> {
        self.clear_path();
        let root = self.root;
        self.descend_to_last(pager, root)
    }
    /// Walk down the right side of the tree, taking the right-most cell of every page.
    fn descend_to_last(&mut self, pager: &mut Pager<V>, page_no: PageNo) -> Result<(), InkError> {
        let mut page_no = page_no;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            let no_of_cells = page.no_of_cells()?;
            let any = AnyPage::parse(
                page_no,
                pager.page_size(),
                pager.usable_size(),
                pager.header_len(),
                page.bytes(),
            )?;

            let child = match &any {
                AnyPage::TableLeaf(_) | AnyPage::IndexLeaf(_) => {
                    self.add_path(page_no, no_of_cells.saturating_sub(1), guard);
                    self.state = CursorState::At;
                    return Ok(());
                }
                AnyPage::TableInterior(p) => p.rmp()?,
                AnyPage::IndexInterior(p) => p.rmp()?,
            };
            self.add_path(page_no, no_of_cells, guard);
            page_no = child;
        }
    }

    /// The cell the cursor is on, read as the cell type it was asked for.
    pub fn current<K: PageKind>(&self, pager: &mut Pager<V>) -> Result<Option<K::Cell>, InkError> {
        if let Some(path) = self.stack.last() {
            let Path {
                page_no,
                cell_idx,
                guard,
                ..
            } = path;

            let inner = page_as_ref_with_pager(*page_no, guard, pager)?;
            let page = TypedPage::<&[u8], K>::wrap(inner);
            if *cell_idx >= page.no_of_cells()? {
                return Ok(None);
            }

            let cell = page.cell(*cell_idx)?;
            return Ok(Some(cell));
        }
        Ok(None)
    }
    /// The largest row id in the tree, which is the row of the last cell of the
    /// last leaf.
    pub fn max_row_id(&mut self, pager: &mut Pager<V>) -> InkResult<u64> {
        self.last(pager)?;
        if let Some(path) = self.stack.last() {
            let page = AnyPage::parse(
                path.page_no,
                pager.page_size(),
                pager.usable_size(),
                pager.header_len(),
                path.guard.bytes(),
            )?;
            let inner = match page {
                AnyPage::TableLeaf(ref inner) => inner,
                AnyPage::TableInterior(_) => {
                    return Err(InkError::Internal("Cursor::last ends in a interior table"));
                }
                _ => return Err(InkError::Internal("Index pages has no RowId")),
            };
            let n_of_cells = inner.no_of_cells()?;
            if n_of_cells == 0 {
                return Ok(0);
            } else {
                let cell = inner.cell(n_of_cells - 1)?;
                return Ok(cell.row_id);
            }
        }
        Err(InkError::Internal(
            "Cusror stack is empty after seeking to last",
        ))
    }
    /// Let go of every page the cursor was holding.
    fn clear_path(&mut self) {
        self.stack.clear();
    }
    /// Note a page and a slot as the cursor goes down.
    fn add_path(&mut self, page_no: PageNo, cell_idx: CellIndex, guard: PageGuard) {
        self.stack.push(Path::new(page_no, cell_idx, guard));
    }

    /// The page the cursor is on, borrowed for as long as the cursor is.
    pub fn current_page_as_ref<'a>(
        &'a self,
        pager: &mut Pager<V>,
    ) -> Result<Option<PageRef<'a>>, InkError> {
        if let Some(path) = self.stack.last() {
            let page = page_as_ref_with_pager(path.page_no, &path.guard, pager)?;
            return Ok(Some(page));
        }
        Ok(None)
    }
    /// The values of the entry the cursor is on.
    ///
    /// A table interior page has no row of its own, so its cell row id is given
    /// back as a one value row.
    pub fn current_record<'a, K: PageKind>(
        &'a self,
        pager: &mut Pager<V>,
    ) -> Result<Option<Vec<Value<'a>>>, InkError>
    where
        K::Cell: HasPayload + Cell,
    {
        let Some(path) = self.stack.last() else {
            return Ok(None);
        };
        let any = AnyPage::parse(
            path.page_no,
            pager.page_size(),
            pager.usable_size(),
            pager.header_len(),
            path.guard.bytes(),
        )?;
        if path.cell_idx >= any.no_of_cells()? {
            return Ok(None);
        }
        let i = path.cell_idx;
        let collected = match &any {
            AnyPage::TableLeaf(p) => Some(p.cell_record(&p.cell(i)?, pager)?),
            AnyPage::IndexLeaf(p) => Some(p.cell_record(&p.cell(i)?, pager)?),
            AnyPage::IndexInterior(p) => Some(p.cell_record(&p.cell(i)?, pager)?),
            AnyPage::TableInterior(p) => {
                let cell = p.cell(i)?;
                Some(vec![Value::Integer(cell.row_id() as i64)])
            }
        };
        Ok(collected.map(|record| record.into_iter().map(|v| v.to_owned_static()).collect()))
    }
    /// The entry the cursor is on as raw record bytes, which is what an index
    /// stores as its key.
    pub fn current_record_bytes(&self, pager: &mut Pager<V>) -> Result<Option<Vec<u8>>, InkError> {
        let Some(path) = self.stack.last() else {
            return Ok(None);
        };
        let any = AnyPage::parse(
            path.page_no,
            pager.page_size(),
            pager.usable_size(),
            pager.header_len(),
            path.guard.bytes(),
        )?;
        if path.cell_idx >= any.no_of_cells()? {
            return Ok(None);
        }
        let i = path.cell_idx;
        Ok(match &any {
            AnyPage::TableLeaf(p) => Some(p.cell_record_as_bytes(&p.cell(i)?, pager)?),
            AnyPage::IndexLeaf(p) => Some(p.cell_record_as_bytes(&p.cell(i)?, pager)?),
            AnyPage::IndexInterior(p) => Some(p.cell_record_as_bytes(&p.cell(i)?, pager)?),
            AnyPage::TableInterior(p) => {
                let cell = p.cell(i)?;
                Some(Tuple::serialize(&[Value::Integer(cell.row_id() as i64)]))
            }
        })
    }

    /// Read a page and look at it through a function.
    #[allow(dead_code)]
    fn with_page<T, FN>(pager: &mut Pager<V>, page_no: PageNo, f: FN) -> Result<T, InkError>
    where
        FN: for<'a> FnOnce(&'a PageRef<'a>) -> Result<T, InkError>,
    {
        let page_guard = pager.get(page_no)?;
        let page = PageRef::new(
            page_no,
            pager.page_size(),
            pager.usable_size(),
            pager.header_len(),
            page_guard.bytes(),
        )?;
        f(&page)
    }
    /// The page and slot the cursor last landed on.
    pub fn last_visited_entry(&self) -> Option<(u32, u16)> {
        if let Some(path) = self.stack.last() {
            return Some((path.page_no, path.cell_idx));
        }
        None
    }
    /// The same, for when the cursor is known to have a position because
    /// something was just sought.
    pub fn last_visited_entry_unchecked(&self) -> (u32, u16) {
        self.last_visited_entry().expect("Path stack is empty")
    }
    /// The last step of the walk down to the cursor.
    pub(crate) fn last_path(&self) -> Option<&Path> {
        self.stack.last()
    }
}

/// What a page tells the cursor to do next as it walks the tree.
enum Step {
    /// Nothing left on this page, so drop it and let the page above decide.
    PopParent,
    /// Carry on within this page.
    Stay { idx: CellIndex, yielded: bool },
    /// Go down into a subtree.
    Descend {
        child: PageNo,
        push_idx: CellIndex,
        push_yielded: bool,
    },
}

/// Walking back through a leaf is just stepping one slot back, or giving up the
/// page when the first cell has been passed.
fn leaf_prev_step(cell_idx: CellIndex) -> Step {
    if cell_idx > 0 {
        Step::Stay {
            idx: cell_idx - 1,
            yielded: false,
        }
    } else {
        Step::PopParent
    }
}
/// The same forward, except that a leaf has no right-most pointer, so the last
/// cell gives up the page.
fn leaf_next_step(cell_index: u16, max: u16) -> Step {
    if cell_index + 1 >= max {
        return Step::PopParent;
    }
    Step::Stay {
        idx: cell_index + 1,
        yielded: false,
    }
}

/// On the way back, an interior table page goes down into the child on its
/// left, since that child holds everything below this cell row id. The first
/// cell has nothing before it, so the page is given up.
fn table_interior_prev_step(
    p: &TypedPage<&[u8], TableInterior>,
    cell_idx: CellIndex,
) -> InkResult<Step> {
    if cell_idx == 0 {
        return Ok(Step::PopParent);
    }
    Ok(Step::Descend {
        child: p.cell(cell_idx - 1)?.left_child(),
        push_idx: cell_idx - 1,
        push_yielded: false,
    })
}
/// On the way forward, an interior table page goes down into the child that
/// follows the current cell. Past the last cell the right-most child is the
/// next subtree, and past that the page is given up.
fn table_interior_next_step(
    p: &TypedPage<&[u8], TableInterior>,
    cell_index: u16,
    max: u16,
) -> InkResult<Step> {
    if cell_index == max {
        return Ok(Step::PopParent);
    }
    if cell_index + 1 == max {
        return Ok(Step::Descend {
            child: p.rmp()?,
            push_idx: max,
            push_yielded: false,
        });
    }
    Ok(Step::Descend {
        child: p.cell(cell_index + 1)?.left_child(),
        push_idx: cell_index + 1,
        push_yielded: false,
    })
}

/// An index interior page holds entries of its own, one between each pair of
/// neighbouring children, so a walk has to hand each one out as it passes
/// through. The `yielded` flag says whether the entry at this slot has been
/// given out already: once it has, going back means going down into the child
/// that sorts before it, and only when that is done does the walk step on to the
/// entry before.
fn index_interior_prev_step(
    p: &TypedPage<&[u8], IndexInterior>,
    cell_idx: CellIndex,
    yielded: bool,
) -> InkResult<Step> {
    if yielded {
        if cell_idx < p.no_of_cells()? {
            return Ok(Step::Descend {
                child: p.cell(cell_idx)?.left_child(),
                push_idx: cell_idx,
                push_yielded: false,
            });
        }
        return Ok(Step::Stay {
            idx: cell_idx - 1,
            yielded: true,
        });
    }
    if cell_idx == 0 {
        return Ok(Step::PopParent);
    }
    Ok(Step::Stay {
        idx: cell_idx - 1,
        yielded: true,
    })
}
/// The mirror of the above: the entry is given out once, before the walk goes
/// down into the child that follows it.
fn index_interior_next_step(
    p: &TypedPage<&[u8], IndexInterior>,
    cell_index: u16,
    max: u16,
    yielded: bool,
) -> InkResult<Step> {
    if cell_index >= max {
        return Ok(Step::PopParent);
    }
    if !yielded {
        return Ok(Step::Stay {
            idx: cell_index,
            yielded: true,
        });
    }
    if cell_index + 1 == max {
        return Ok(Step::Descend {
            child: p.rmp()?,
            push_idx: max,
            push_yielded: false,
        });
    }
    Ok(Step::Descend {
        child: p.cell(cell_index + 1)?.left_child(),
        push_idx: cell_index + 1,
        push_yielded: false,
    })
}

/// Find a row id in a page with a binary search.
///
/// The answer is whether it was found and the slot it is at, or the slot
/// just after where it would have gone. Only a leaf can report a row id as
/// found, since the same row id on an interior page is a divider and not a
/// row.
pub(crate) fn search_row_ids<B: AsRef<[u8]>, K: PageKind, V: Vfs>(
    page: &TypedPage<B, K>,
    _pager: &mut Pager<V>,
    target: u64,
) -> InkResult<(bool, u16)>
where
    K::Cell: HasRowId,
{
    let cell_cnt = page.no_of_cells()?;
    let mut l = 0;
    let mut r = cell_cnt;
    while l < r {
        let m: u16 = l + ((r - l) / 2);
        let row_id = page.cell(m)?.row_id();
        if row_id == target && K::IS_LEAF {
            return Ok((true, m));
        } else if row_id >= target {
            r = m;
        } else {
            l = m + 1;
        }
    }
    Ok((false, l))
}
/// Find an index entry in a page with a binary search.
///
/// An entry that matches only the columns the key names counts as an equal
/// prefix and is remembered, since a later cell may match more of the
/// leading run. When no cell matches more than that, the first one that did
/// is the answer.
/// # Example
/// Imagine we have an index on `name`, so the tree looks something like this:
/// ```text
///                     +----+------+-----+
///                    /| P0 |David | 42  |
///                   / +----+------+-----+\
///                  /  | P1 | leam | 82  | \
///                 /   +--+-+------+-----+  \
///        /-------/       |                  \---\
///       /                +---+                   \
///      /                     |                    \  RMP
///     v                      v                     v
/// +------+-----+         +------+-----+        +------+-----+
/// |Alice | 10  |         | Eve  | 51  |        |Ivan  | 91  |
/// +------+-----+         +------+-----+        +------+-----+
/// |Alice | 20  |         |Terry | 60  |        |Judy  | 97  |
/// +------+-----+         +------+-----+        +------+-----+
/// | Bob  | 30  |         |Davis | 70  |        |Karl  | 99  |
/// +------+-----+         +------+-----+        +------+-----+
/// |Carol | 40  |
/// +------+-----+
///
/// ```
/// We want to search for the name "Alice", but there are two entries with the
/// same name. A normal seek could stop at the second "Alice" and return
/// ["Alice", 20]. Since we are searching by prefix, we keep searching until
/// the loop ends, leaving the cursor at the first "Alice" or where it would be
/// if no matching entry exists.s
/// ```text
///                                         +----+------+-----+
///                                        /| P0 |David | 42  |
///                                       / +----+------+-----+\
///                                      /  | P1 | leam | 82  | \
///                                     /   +--+-+------+-----+  \
///                            /-------/       |                  \---\
///                           /                +---+                   \
///                          /                     |                    \
///                         v                      v                     v
///                     +------+-----+         +------+-----+        +------+-----+
/// Search stopped ---->|Alice | 10  |         | Eve  | 51  |        |Ivan  | 91  |
///      here           +------+-----+         +------+-----+        +------+-----+
///                     |Alice | 20  |         |Terry | 60  |        |Judy  | 97  |
///                     +------+-----+         +------+-----+        +------+-----+
///                     | Bob  | 30  |         |Davis | 70  |        |Karl  | 99  |
///                     +------+-----+         +------+-----+        +------+-----+
///                     |Carol | 40  |
///                     +------+-----+
///
/// ```
///
/// But if we have the full key, such as `["Alice", 20]`, the seek stops as
/// soon as it finds an exact match.
///
pub(crate) fn search_indexes<B: AsRef<[u8]>, K: IndexKind, V: Vfs>(
    page: &TypedPage<B, K>,
    pager: &mut Pager<V>,
    target: &Value,
) -> InkResult<IndexSearchResult>
where
    K::Cell: HasPayload,
{
    assert!(
        K::IS_INDEX,
        "SearchIndexes function called with an TableLeaf Cell"
    );

    let cell_count = page.no_of_cells()?;
    let mut l = 0;
    let mut r = cell_count;
    let mut found = None;
    while l < r {
        let m = l + (r - l) / 2;
        let cell = page.cell(m)?;
        let entry = page.cell_record(&cell, pager)?;
        let (ord, full) = compare_index_entry(&entry, target)?;
        if ord == Ordering::Equal {
            if full {
                return Ok(IndexSearchResult::Exact(m));
            }
            found = Some(m);
            r = m;
            continue;
        }
        if ord == Ordering::Greater {
            r = m;
        } else {
            l = m + 1;
        }
    }
    if let Some(m) = found {
        return Ok(EqualPrefix(m));
    }
    Ok(NotFound(l))
}

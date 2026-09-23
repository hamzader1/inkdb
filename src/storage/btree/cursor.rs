use super::{CellIndex, page_as_ref_with_pager};
use super::{binary_search_interior, binary_search_leaf};
use crate::SqliteError;
use crate::SqliteResult;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::SqlType;
use crate::record::Value;
use crate::storage::cell::BTreeCell;
use crate::storage::page::{BTreePage, BTreePageType, PageRef as BTreePageRef};
use crate::util::sqlite_assert_with_corrupt_err;
use std::cmp::Ordering;

/// Prefix comparison of an index entry `[key…, rowid]` against a seek
/// target (`Tuple(key…)`). Only the overlapping positions decide: extra
/// trailing entry elements (the rowid) never participate unless the target
/// covers them too.
/// Returns the ordering plus whether the target covered the whole entry —
/// a full hit is a unique entry, a prefix hit sits inside a duplicate run.

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum CursorState {
    At,
    Invalid,
    AfterLast,
    BeforeFirst,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SeekResult {
    Exact,
    NotFound,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum UnderflowAction {
    BorrowLeft,
    BorrowRight,
}

#[derive(Debug)]
pub struct Path {
    pub page_no: PageNo,
    pub cell_idx: u16,
    pub(crate) guard: PageGuard,
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

    pub(crate) fn guard(&self) -> &PageGuard {
        &self.guard
    }
}

pub enum SearchResult {
    Descend {
        child: u32,
        cell_index: CellIndex,
        /// Whether an equal key was seen during the probe. Table probes
        /// never set this; index probes set it when a divider equals the
        /// target (duplicates may still live in leaves below).
        exact: bool,
    },
}
impl SearchResult {
    pub fn cell_index(&self) -> CellIndex {
        match self {
            Self::Descend { cell_index, .. } => *cell_index,
        }
    }
}

pub enum RestorePosition {
    Exact,
    Next,
    Empty,
}

#[derive(Debug)]
pub struct BTreeCursor<V: crate::vfs::Vfs> {
    pub root: PageNo,
    pub stack: Vec<Path>,
    pub state: CursorState,
    pub saved_key: Option<Value<'static>>,
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
     * Optimaze these two functions below
     */
    pub fn save_position(&mut self, pager: &mut Pager<V>) -> SqliteResult<()> {
        if let Some(last_entry) = self.stack.last() {
            let guard = pager.get(last_entry.page_no)?;
            let page = page_as_ref_with_pager(last_entry.page_no, &guard, pager)?;
            if last_entry.cell_idx >= page.no_of_cells()? {
                // Past the end
                // No key to save
                self.saved_key = None;
                self.saved_yielded = false;
                self.stack.clear();
                return Ok(());
            }
            self.saved_yielded = last_entry.yielded;
            let cell = page.cell(last_entry.cell_idx)?;
            let key = page.cell_key(&cell, pager)?;
            self.saved_key = Some(key);
            self.stack.clear();
        }
        Ok(())
    }

    pub fn restore_position(&mut self, pager: &mut Pager<V>) -> SqliteResult<RestorePosition> {
        let Some(key) = self.saved_key.take() else {
            return Ok(RestorePosition::Empty);
        };
        let saved_yielded = self.saved_yielded;
        self.saved_yielded = false;
        self.seek_internal(pager, &key, true)?;
        let Some(path) = self.stack.last_mut() else {
            return Ok(RestorePosition::Next);
        };
        if saved_yielded {
            // A saved interior divider was already yielded once, so park it
            // as yielded again. Otherwise the caller would visit it twice.
            let guard = pager.get(path.page_no)?;
            let page = page_as_ref_with_pager(path.page_no, &guard, pager)?;
            if !page.is_leaf()? {
                path.yielded = true;
            }
        }
        let (page_no, cell_idx) = (path.page_no, path.cell_idx);
        let guard = pager.get(page_no)?;
        let page = page_as_ref_with_pager(page_no, &guard, pager)?;
        if cell_idx < page.no_of_cells()? {
            let cell = page.cell(cell_idx)?;
            if page.cell_key(&cell, pager)? == key {
                return Ok(RestorePosition::Exact);
            }
        }
        // The saved key is gone (it was just deleted). The seek above may
        // have parked past the end of a leaf. Step forward so the cursor
        // sits on the real successor instead of dead space.
        self.skip_past_end(pager)?;
        Ok(RestorePosition::Next)
    }

    /// Move past a past the end leaf position onto the next real entry.
    /// A seek for a deleted key lands where the key would go, which is
    /// often cell_idx == no_of_cells. Calling next from there climbs and
    /// descends into the following leaf, or parks AfterLast when done.
    /// Insert never uses this since it needs the raw landing spot.
    pub fn skip_past_end(&mut self, pager: &mut Pager<V>) -> SqliteResult<()> {
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

    /// Seek to the first entry at or after target. Same as seek but a
    /// past the end landing is advanced to the next leaf for callers
    /// that iterate forward.
    pub fn seek_lower_bound(
        &mut self,
        pager: &mut Pager<V>,
        target: &Value<'_>,
    ) -> SqliteResult<SeekResult> {
        let res = self.seek_internal(pager, target, true)?;
        self.skip_past_end(pager)?;
        Ok(res)
    }

    pub fn is_valid(&self) -> bool {
        self.state == CursorState::At && !self.stack.is_empty()
    }

    pub fn seek(
        &mut self,
        pager: &mut Pager<V>,
        target: &Value<'_>,
    ) -> Result<SeekResult, SqliteError> {
        self.seek_internal(pager, target, false)
    }

    /// Seek that stops on an interior divider when the full key matches.
    /// Delete needs this because an index entry may live only in the
    /// parent. Insert and prefix scans keep using plain seek.
    pub fn seek_for_delete(
        &mut self,
        pager: &mut Pager<V>,
        target: &Value<'_>,
    ) -> Result<SeekResult, SqliteError> {
        self.seek_internal(pager, target, true)
    }

    fn seek_internal(
        &mut self,
        pager: &mut Pager<V>,
        target: &Value<'_>,
        stop_at_interior: bool,
    ) -> Result<SeekResult, SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        let mut exact = false;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf()? {
                let (found, cell_idx) = binary_search_leaf(&page, pager, target)?;
                self.stack.push(Path::new(page_no, cell_idx, guard));
                if found {
                    exact = true;
                }
                if exact {
                    return Ok(SeekResult::Exact);
                }
                return Ok(SeekResult::NotFound);
            }
            self.state = CursorState::At;
            let search_result = binary_search_interior(&page, pager, target)?;
            match search_result {
                SearchResult::Descend {
                    child,
                    cell_index,
                    exact: saw_eq,
                } => {
                    if stop_at_interior && saw_eq && cell_index < page.no_of_cells()? {
                        let cell = page.cell(cell_index)?;
                        let key = page.cell_key(&cell, pager)?;
                        if key == *target {
                            self.stack.push(Path::new(page_no, cell_index, guard));
                            return Ok(SeekResult::Exact);
                        }
                    }
                    // Parked unyielded: forward iteration yields this divider
                    // itself when the left subtree is exhausted.
                    self.stack.push(Path::new(page_no, cell_index, guard));
                    page_no = child;
                    exact |= saw_eq;
                }
            }
        }
    }

    pub fn next(&mut self, pager: &mut Pager<V>) -> Result<(), SqliteError> {
        while let Some(path) = self.stack.pop() {
            let Path {
                page_no,
                cell_idx,
                guard,
                yielded: yeilded,
            } = path;

            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf()? {
                if cell_idx + 1 < page.no_of_cells()? {
                    self.stack.push(Path::new(page_no, cell_idx + 1, guard));
                    self.state = CursorState::At;
                    return Ok(());
                }
            } else {
                if page.page_type()? == BTreePageType::InteriorIndex
                    && !yeilded
                    && cell_idx < page.no_of_cells()?
                {
                    self.stack.push(Path {
                        page_no,
                        cell_idx,
                        guard,
                        yielded: true,
                    });
                    self.state = CursorState::At;
                    return Ok(());
                } else if cell_idx + 1 == page.no_of_cells()? {
                    let child = page.right_most_ptr()?.ok_or(SqliteError::Internal(format!(
                        "cursor next: interior page {page_no} has no right-most child"
                    )))?;
                    self.add_path(page_no, cell_idx + 1, guard);
                    self.descend_to_first(pager, child)?;
                    self.state = CursorState::At;
                    return Ok(());
                } else if cell_idx + 1 < page.no_of_cells()? {
                    let child = page.cell(cell_idx + 1)?.left_child();
                    self.add_path(page_no, cell_idx + 1, guard);
                    self.descend_to_first(pager, child)?;
                    self.state = CursorState::At;
                    return Ok(());
                }
            }
        }
        self.state = CursorState::AfterLast;
        Ok(())
    }
    pub fn first(&mut self, pager: &mut Pager<V>) -> Result<(), SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf()? {
                self.add_path(page_no, 0, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.cell(0)?.left_child();
            self.add_path(page_no, 0, guard);
            page_no = child;
        }
    }
    pub fn descend_to_first(
        &mut self,
        pager: &mut Pager<V>,
        page_no: PageNo,
    ) -> Result<(), SqliteError> {
        let mut page_no = page_no;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf()? {
                self.add_path(page_no, 0, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.cell(0)?.left_child();
            self.add_path(page_no, 0, guard);
            page_no = child;
        }
    }
    pub fn prev(&mut self, pager: &mut Pager<V>) -> Result<(), SqliteError> {
        while let Some(path) = self.stack.pop() {
            let Path {
                page_no,
                cell_idx,
                guard,
                yielded: yeilded,
            } = path;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf()? {
                if cell_idx > 0 {
                    self.add_path(page_no, cell_idx - 1, guard);
                    self.state = CursorState::At;
                    return Ok(());
                }
            } else if page.page_type()? == BTreePageType::InteriorIndex {
                if yeilded {
                    if cell_idx < page.no_of_cells()? {
                        let child = page.cell(cell_idx)?.left_child();
                        self.stack.push(Path::new(page_no, cell_idx, guard));
                        self.descend_to_last(pager, child)?;
                        self.state = CursorState::At;
                        return Ok(());
                    }
                    self.stack.push(Path {
                        page_no,
                        cell_idx: cell_idx - 1,
                        guard,
                        yielded: true,
                    });
                    self.state = CursorState::At;
                    return Ok(());
                }
                if cell_idx == 0 {
                    continue;
                }
                self.stack.push(Path {
                    page_no,
                    cell_idx: cell_idx - 1,
                    guard,
                    yielded: true,
                });
                self.state = CursorState::At;
                return Ok(());
            } else if cell_idx > 0 {
                let child = page.cell(cell_idx - 1)?.left_child();
                self.add_path(page_no, cell_idx - 1, guard);
                self.descend_to_last(pager, child)?;
                self.state = CursorState::At;
                return Ok(());
            }
        }
        self.state = CursorState::BeforeFirst;
        Ok(())
    }
    pub fn last(&mut self, pager: &mut Pager<V>) -> Result<(), SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf()? {
                let cell_idx = if page.no_of_cells()? == 0 {
                    0
                } else {
                    page.no_of_cells()? - 1
                };
                self.add_path(page_no, cell_idx, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.right_most_ptr()?.ok_or(SqliteError::Internal(format!(
                "cursor last: interior page {page_no} has no right-most child"
            )))?;
            self.add_path(page_no, page.no_of_cells()?, guard);
            page_no = child;
        }
    }
    fn descend_to_last(
        &mut self,
        pager: &mut Pager<V>,
        page_no: PageNo,
    ) -> Result<(), SqliteError> {
        let mut page_no = page_no;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf()? {
                self.add_path(page_no, page.no_of_cells()? - 1, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.right_most_ptr()?.ok_or(SqliteError::Internal(format!(
                "cursor descend_to_last: interior page {page_no} has no right-most child"
            )))?;
            self.add_path(page_no, page.no_of_cells()?, guard);
            page_no = child;
        }
    }

    pub fn current(&self, pager: &mut Pager<V>) -> Result<Option<BTreeCell>, SqliteError> {
        if let Some(path) = self.stack.last() {
            let Path {
                page_no,
                cell_idx,
                guard,
                ..
            } = path;

            let page = page_as_ref_with_pager(*page_no, guard, pager)?;
            if *cell_idx >= page.no_of_cells()? {
                return Ok(None);
            }
            let cell = page.cell(*cell_idx)?;
            return Ok(Some(cell));
        }
        Ok(None)
    }
    fn clear_path(&mut self) {
        self.stack.clear();
    }

    pub fn last_visited_entry(&self) -> Option<(u32, u16)> {
        if let Some(path) = self.stack.last() {
            return Some((path.page_no, path.cell_idx));
        }
        None
    }
    pub fn last_visited_entry_unchecked(&self) -> (u32, u16) {
        self.last_visited_entry().expect("Path stack is empty")
    }

    pub fn current_page_as_ref<'a>(
        &'a self,
        pager: &mut Pager<V>,
    ) -> Result<Option<BTreePageRef<'a>>, SqliteError> {
        if let Some(path) = self.stack.last() {
            let page = page_as_ref_with_pager(path.page_no, &path.guard, pager)?;
            return Ok(Some(page));
        }
        Ok(None)
    }
    pub fn current_record<'a>(
        &'a self,
        pager: &mut Pager<V>,
    ) -> Result<Option<Vec<Value<'a>>>, SqliteError> {
        if let Some(page) = self.current_page_as_ref(pager)?
            && let Some(cell) = self.current(pager)?
        {
            let record = page.record_of(&cell, pager)?;
            return Ok(Some(record.into_iter().map(|v| v.into_owned()).collect()));
        }
        Ok(None)
    }

    fn with_page<T, FN>(pager: &mut Pager<V>, page_no: PageNo, f: FN) -> Result<T, SqliteError>
    where
        FN: for<'a> FnOnce(&'a BTreePageRef<'a>) -> Result<T, SqliteError>,
    {
        let page_guard = pager.get(page_no)?;
        let page = BTreePageRef::new(
            page_no,
            pager.page_size(),
            pager.usable_size(),
            page_guard.bytes(),
        )?;
        f(&page)
    }
    pub fn with_current<FN, R>(&mut self, pager: &mut Pager<V>, f: FN) -> Result<R, SqliteError>
    where
        FN: for<'a> FnOnce(&'a BTreePageRef<'a>, &'a BTreeCell) -> Result<R, SqliteError>,
    {
        let path = self.stack.last().unwrap();
        let Path {
            page_no, cell_idx, ..
        } = path;
        Self::with_page(pager, *page_no, |page| {
            let cell = page.cell(*cell_idx)?;
            f(page, &cell)
        })
    }

    fn add_path(&mut self, page_no: PageNo, cell_idx: CellIndex, guard: PageGuard) {
        self.stack.push(Path::new(page_no, cell_idx, guard));
    }
}

use std::cmp::Ordering;

use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::btree::{CellIndex, compare_index_entry, page_as_ref_with_pager};
use crate::storage::btree_remake::kind::HasChild;
use crate::storage::page::{BTreePageType, PageRef};
use crate::vfs::Vfs;

use self::IndexSearchResult::{EqualPrefix, NotFound};

use super::kind::{AnyPage, Cell, HasPayload, HasRowId, PageKind, TypedPage};

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

pub enum RestorePosition {
    Exact,
    Next,
    Empty,
}

pub enum IndexSearchResult {
    Exact(u16),
    EqualPrefix(u16),
    NotFound(u16), // hint
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
            // let inner = page_as_ref_with_pager(last_entry.page_no, &guard, pager)?;
            let page = AnyPage::parse(
                last_entry.page_no,
                pager.page_size(),
                pager.usable_size(),
                guard.bytes(),
            )?;
            if last_entry.cell_idx >= page.no_of_cells()? {
                // Past the end
                // No key to save
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
        let page = AnyPage::parse(
            page_no,
            pager.page_size(),
            pager.usable_size(),
            guard.bytes(),
        )?;
        if cell_idx < page.no_of_cells()? {
            let i = cell_idx;
            let cell_key = page.cell_key(i, pager)?;

            if cell_key == key {
                return Ok(RestorePosition::Exact);
            }
        }
        // The saved key is gone (it was just deleted). The seek above may
        // have parked past the end of a leaf. Step forward so the cursor
        // sits on the real successor instead of dead space.
        // self.skip_past_end(pager)?;
        Ok(RestorePosition::Next)
    }

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
        target: &Value,
        stop_at_interior: bool,
    ) -> SqliteResult<SeekResult> {
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            match AnyPage::parse(
                page_no,
                pager.page_size(),
                pager.usable_size(),
                guard.bytes(),
            )? {
                AnyPage::TableInterior(ref p) => {
                    let (_, i) = search_row_ids(p, pager, target.cast_int()? as _)?;
                    page_no = if i < p.no_of_cells()? {
                        p.cell(i)?.left_child()
                    } else {
                        p.rmp()?
                    };
                    self.stack.push(Path::new(page_no, i, guard));
                    self.state = CursorState::At;
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
                        page_no = if i < p.no_of_cells()? {
                            p.cell(i)?.left_child()
                        } else {
                            p.rmp()?
                        };
                        self.stack.push(Path::new(page_no, i, guard));
                        self.state = CursorState::At;
                    }
                },
                AnyPage::IndexLeaf(ref p) => match search_indexes(p, pager, target)? {
                    IndexSearchResult::Exact(i) | IndexSearchResult::EqualPrefix(i) => {
                        self.state = CursorState::At;
                        self.stack.push(Path::new(page_no, i, guard));
                        return Ok(SeekResult::Exact);
                    }
                    IndexSearchResult::NotFound(i) => {
                        // has no rmp
                        self.state = CursorState::At;
                        self.stack.push(Path::new(page_no, i, guard));
                        return Ok(SeekResult::NotFound);
                    }
                },
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

    pub fn current<K: PageKind>(
        &self,
        pager: &mut Pager<V>,
    ) -> Result<Option<K::Cell>, SqliteError> {
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
    fn clear_path(&mut self) {
        self.stack.clear();
    }
    fn add_path(&mut self, page_no: PageNo, cell_idx: CellIndex, guard: PageGuard) {
        self.stack.push(Path::new(page_no, cell_idx, guard));
    }

    pub fn current_page_as_ref<'a>(
        &'a self,
        pager: &mut Pager<V>,
    ) -> Result<Option<PageRef<'a>>, SqliteError> {
        if let Some(path) = self.stack.last() {
            let page = page_as_ref_with_pager(path.page_no, &path.guard, pager)?;
            return Ok(Some(page));
        }
        Ok(None)
    }
    pub fn current_record<'a, K: PageKind>(
        &'a self,
        pager: &mut Pager<V>,
    ) -> Result<Option<Vec<Value<'a>>>, SqliteError>
    where
        K::Cell: HasPayload + Cell,
    {
        if let Some(page) = self.current_page_as_ref(pager)?
            && let Some(cell) = self.current::<K>(pager)?
        {
            let record = page.get_cell_record_v2(cell, pager)?;
            return Ok(Some(record.into_iter().map(|v| v.into_owned()).collect()));
        }
        Ok(None)
    }
    fn with_page<T, FN>(pager: &mut Pager<V>, page_no: PageNo, f: FN) -> Result<T, SqliteError>
    where
        FN: for<'a> FnOnce(&'a PageRef<'a>) -> Result<T, SqliteError>,
    {
        let page_guard = pager.get(page_no)?;
        let page = PageRef::new(
            page_no,
            pager.page_size(),
            pager.usable_size(),
            page_guard.bytes(),
        )?;
        f(&page)
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
}
// fn foo() {}

fn search_row_ids<B: AsRef<[u8]>, K: PageKind, V: Vfs>(
    page: &TypedPage<B, K>,
    pager: &mut Pager<V>,
    target: u64,
) -> SqliteResult<(bool, u16)>
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
fn search_indexes<B: AsRef<[u8]>, K: PageKind, V: Vfs>(
    page: &TypedPage<B, K>,
    pager: &mut Pager<V>,
    target: &Value,
) -> SqliteResult<IndexSearchResult>
where
    K::Cell: HasPayload,
{
    // Since TableLeafCell do met the Cell requirement
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
        let entry = page.get_cell_record_v2(cell, pager)?;
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

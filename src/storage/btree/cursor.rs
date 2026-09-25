use super::kind::HasChild;
use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::btree::{CellIndex, compare_index_entry, page_as_ref_with_pager};
use crate::storage::page::{BTreePageType, PageRef};
use crate::vfs::Vfs;
use std::cmp::Ordering;

use self::IndexSearchResult::{EqualPrefix, NotFound};

use super::kind::{
    AnyPage, Cell, HasPayload, HasRowId, IndexInterior, IndexKind, PageKind, TableInterior,
    TypedPage,
};

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
    NotFound(u16),
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
            let page = AnyPage::parse(
                last_entry.page_no,
                pager.page_size(),
                pager.usable_size(),
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
            let guard = pager.get(path.page_no)?;
            let any = AnyPage::parse(
                path.page_no,
                pager.page_size(),
                pager.usable_size(),
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
            guard.bytes(),
        )?;
        if cell_idx < page.no_of_cells()? {
            let i = cell_idx;
            let cell_key = page.cell_key(i, pager)?;

            if cell_key == key {
                return Ok(RestorePosition::Exact);
            }
        }
        self.skip_past_end(pager)?;
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
        self.clear_path();
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
    pub fn next(&mut self, pager: &mut Pager<V>) -> SqliteResult<()> {
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

    pub fn descend_to_first(
        &mut self,
        pager: &mut Pager<V>,
        page_no: PageNo,
    ) -> Result<(), SqliteError> {
        let mut page_no = page_no;
        loop {
            let guard = pager.get(page_no)?;
            let page = AnyPage::parse(
                page_no,
                pager.page_size(),
                pager.usable_size(),
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
    pub fn first(&mut self, pager: &mut Pager<V>) -> Result<(), SqliteError> {
        self.clear_path();
        let root = self.root;
        self.descend_to_first(pager, root)
    }

    pub fn prev(&mut self, pager: &mut Pager<V>) -> Result<(), SqliteError> {
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

    pub fn last(&mut self, pager: &mut Pager<V>) -> Result<(), SqliteError> {
        self.clear_path();
        let root = self.root;
        self.descend_to_last(pager, root)
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
            let no_of_cells = page.no_of_cells()?;
            let any = AnyPage::parse(
                page_no,
                pager.page_size(),
                pager.usable_size(),
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
        let Some(path) = self.stack.last() else {
            return Ok(None);
        };
        let any = AnyPage::parse(
            path.page_no,
            pager.page_size(),
            pager.usable_size(),
            path.guard.bytes(),
        )?;
        if path.cell_idx >= any.no_of_cells()? {
            return Ok(None);
        }
        let i = path.cell_idx;
        let collected = match &any {
            AnyPage::TableLeaf(p) => Some(p.get_cell_record_v2(&p.cell(i)?, pager)?),
            AnyPage::IndexLeaf(p) => Some(p.get_cell_record_v2(&p.cell(i)?, pager)?),
            AnyPage::IndexInterior(p) => Some(p.get_cell_record_v2(&p.cell(i)?, pager)?),
            AnyPage::TableInterior(p) => {
                let cell = p.cell(i)?;
                Some(vec![Value::Integer(cell.row_id() as i64)])
            }
        };
        Ok(collected.map(|record| record.into_iter().map(|v| v.to_owned_static()).collect()))
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

enum Step {
    PopParent,
    Stay {
        idx: CellIndex,
        yielded: bool,
    },
    Descend {
        child: PageNo,
        push_idx: CellIndex,
        push_yielded: bool,
    },
}

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
fn leaf_next_step(cell_index: u16, max: u16) -> Step {
    if cell_index + 1 >= max {
        return Step::PopParent;
    }
    Step::Stay {
        idx: cell_index + 1,
        yielded: false,
    }
}

fn table_interior_prev_step(
    p: &TypedPage<&[u8], TableInterior>,
    cell_idx: CellIndex,
) -> SqliteResult<Step> {
    if cell_idx == 0 {
        return Ok(Step::PopParent);
    }
    Ok(Step::Descend {
        child: p.cell(cell_idx - 1)?.left_child(),
        push_idx: cell_idx - 1,
        push_yielded: false,
    })
}
fn table_interior_next_step(
    p: &TypedPage<&[u8], TableInterior>,
    cell_index: u16,
    max: u16,
) -> SqliteResult<Step> {
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

fn index_interior_prev_step(
    p: &TypedPage<&[u8], IndexInterior>,
    cell_idx: CellIndex,
    yielded: bool,
) -> SqliteResult<Step> {
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
fn index_interior_next_step(
    p: &TypedPage<&[u8], IndexInterior>,
    cell_index: u16,
    max: u16,
    yielded: bool,
) -> SqliteResult<Step> {
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

pub(crate) fn search_row_ids<B: AsRef<[u8]>, K: PageKind, V: Vfs>(
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
pub(crate) fn search_indexes<B: AsRef<[u8]>, K: IndexKind, V: Vfs>(
    page: &TypedPage<B, K>,
    pager: &mut Pager<V>,
    target: &Value,
) -> SqliteResult<IndexSearchResult>
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
        let entry = page.get_cell_record_v2(&cell, pager)?;
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

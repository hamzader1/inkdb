pub mod cursor;
pub mod delete;
pub mod insert;
pub mod rebalance;
pub mod split;

use std::cmp::Ordering;

pub use cursor::{BTreeCursor, CursorState, Path, RestorePosition, SeekResult};

use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::cell::BTreeCell;
use crate::util::sqlite_assert_with_corrupt_err;
use crate::vfs::Vfs;

use self::cursor::SearchResult;

pub use super::page::PageRef;
pub use super::page::PageRef as BTreePageRef;
use super::page::{BTreePage, BTreePageType, PageMut as BTreePageMut};

pub type CellIndex = u16;

pub fn page_as_ref_with_pager<'b, V: crate::vfs::Vfs>(
    page_no: PageNo,
    guard: &'b PageGuard,
    pager: &Pager<V>,
) -> Result<PageRef<'b>, SqliteError> {
    PageRef::new(
        page_no,
        pager.page_size(),
        pager.usable_size(),
        guard.bytes(),
    )
}

pub fn page_as_mut_with_pager<'b, V: crate::vfs::Vfs>(
    page_no: PageNo,
    guard: &'b mut PageGuard,
    pager: &Pager<V>,
) -> Result<BTreePageMut<'b>, SqliteError> {
    BTreePageMut::new(
        page_no,
        pager.page_size(),
        pager.usable_size(),
        guard.bytes_as_mut().unwrap(),
    )
}
pub struct BTree<'a, V: crate::vfs::Vfs> {
    pub root_page: PageNo,
    pub pager: &'a mut Pager<V>,
    pub cursor: BTreeCursor<V>,
}

#[derive(Debug, Clone)]
pub struct SplitMetadata {
    pub left_page: u32,
    pub right_page: u32,
    pub boundary: Value<'static>,
    pub boundary_bytes: Option<Vec<u8>>,
    pub right_max: Value<'static>,
    pub right_max_bytes: Option<Vec<u8>>,
}
impl SplitMetadata {
    pub fn new(
        left_page: u32,
        right_page: u32,
        boundary: Value<'static>,
        right_max: Value<'static>,
    ) -> Self {
        Self {
            left_page,
            right_page,
            boundary,
            right_max,
            boundary_bytes: None,
            right_max_bytes: None,
        }
    }
}
impl<'a, V: crate::vfs::Vfs> BTree<'a, V> {
    pub fn new(root_page: PageNo, pager: &'a mut Pager<V>) -> Self {
        Self {
            root_page,
            pager,
            cursor: BTreeCursor::new(root_page),
        }
    }
    pub fn with_cursor(pager: &'a mut Pager<V>, cursor: BTreeCursor<V>) -> Self {
        Self {
            root_page: cursor.root,
            pager,
            cursor,
        }
    }
    pub fn seek(&mut self, target: &Value) -> SqliteResult<SeekResult> {
        self.cursor.seek(self.pager, target)
    }
    /*
     * TODO: REMOVE THIS
     */
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> SqliteResult<()> {
        self.cursor.next(self.pager)
    }
    pub fn prev(&mut self) -> SqliteResult<()> {
        self.cursor.prev(self.pager)
    }

    pub fn current_record(&mut self) -> SqliteResult<Option<Vec<Value<'_>>>> {
        self.cursor.current_record(self.pager)
    }
    pub fn page_as_ref(
        &self,
        page_no: PageNo,
        guard: &'a PageGuard,
    ) -> Result<BTreePageRef<'a>, SqliteError> {
        BTreePageRef::new(
            page_no,
            self.pager.page_size(),
            self.pager.usable_size(),
            guard.bytes(),
        )
    }

    pub fn page_as_mut(
        &self,
        page_no: PageNo,
        guard: &'a mut PageGuard,
    ) -> Result<BTreePageMut<'a>, SqliteError> {
        BTreePageMut::new(
            page_no,
            self.pager.page_size(),
            self.pager.usable_size(),
            guard.bytes_as_mut().unwrap(),
        )
    }

    pub fn with_page_ref<Func, R>(&mut self, page_no: PageNo, f: Func) -> Result<R, SqliteError>
    where
        Func: FnOnce(&BTreePageRef) -> Result<R, SqliteError>,
    {
        let guard = self.pager.get(page_no)?;
        let p = self.page_as_ref(page_no, &guard)?;
        f(&p)
    }

    pub fn with_page_mut<Func, R>(&mut self, page_no: PageNo, f: Func) -> Result<R, SqliteError>
    where
        Func: for<'b> FnOnce(&'b mut BTreePageMut) -> Result<R, SqliteError>,
    {
        let mut guard = self.pager.get_mut(page_no)?;
        let mut p = self.page_as_mut(page_no, &mut guard)?;
        f(&mut p)
    }
    pub fn with_page_cell_mut<Func, R>(
        &mut self,
        page_no: u32,
        cell_index: u16,
        f: Func,
    ) -> Result<R, SqliteError>
    where
        Func: FnOnce(&mut BTreePageMut, &BTreeCell) -> Result<R, SqliteError>,
    {
        // self.with_page_mut(page_no, |page|)
        let mut guard = self.pager.get_mut(page_no)?;
        let mut page = self.page_as_mut(page_no, &mut guard)?;
        let cell = page.cell(cell_index)?;
        f(&mut page, &cell)
    }

    pub fn allocate_page(&mut self) -> Result<PageNo, SqliteError> {
        self.pager.allocate_new_page()
    }
    pub fn deallocate_page(&mut self, page_no: PageNo) -> SqliteResult<()> {
        self.pager.dealloc(page_no)
    }

    pub fn seek_into_first(&mut self) -> Result<(), SqliteError> {
        self.cursor.first(self.pager)
    }
    pub fn seek_into_last(&mut self) -> Result<(), SqliteError> {
        self.cursor.last(self.pager)
    }
    pub fn current_page_header_unchecked(&mut self) -> Result<u16, SqliteError> {
        let (pn, _) = self.cursor.last_visited_entry_unchecked();

        self.with_page_ref(pn, |page| page.no_of_cells())
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum UnderflowAction {
    BorrowLeft,
    BorrowRight,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ActivePath {
    page_no: PageNo,
    cell_idx: CellIndex,
}
impl From<&Vec<Path>> for ActivePath {
    fn from(value: &Vec<Path>) -> Self {
        let Path {
            page_no, cell_idx, ..
        } = value.last().unwrap();
        Self {
            page_no: *page_no,
            cell_idx: *cell_idx,
        }
    }
}

pub(crate) fn binary_search_leaf<B: AsRef<[u8]>, V: Vfs>(
    page: &BTreePage<B>,
    pager: &mut Pager<V>,
    target: &Value<'_>,
) -> Result<(bool, CellIndex), SqliteError> {
    sqlite_assert_with_corrupt_err(page.is_leaf()?, || {
        "This navigation path works only for leaves".into()
    })?;

    let cell_cnt = page.no_of_cells()?;
    let mut l = 0;
    let mut r = cell_cnt;
    let mut found = None;
    while l < r {
        let m: u16 = l + ((r - l) / 2);

        let value = if page.page_type()? == BTreePageType::LeafTable {
            Value::Integer(page.cell(m)?.row_id() as _)
        } else {
            let entry = page.record_of_cell(m, pager)?;
            let (ord, full) = compare_index_entry(&entry, target)?;
            if ord == Ordering::Equal {
                if full {
                    return Ok((true, m));
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
            continue;
        };

        if &value == target {
            found = Some(m);
            r = m;
        } else if &value > target {
            r = m;
        } else {
            l = m + 1;
        }
    }

    if let Some(m) = found {
        return Ok((true, m));
    }
    Ok((false, l))
}

pub(crate) fn binary_search_interior<B: AsRef<[u8]>, V: Vfs>(
    page: &BTreePage<B>,
    pager: &mut Pager<V>,
    target: &Value,
) -> Result<SearchResult, SqliteError> {
    sqlite_assert_with_corrupt_err(page.is_interior()?, || {
        "Navigation path of this works only with interior pages".into()
    })?;

    let cell_count = page.no_of_cells()?;
    let is_table = page.page_type()? == BTreePageType::InteriorTable;

    let mut l = 0;
    let mut r = cell_count;
    let mut saw_eq = false;

    while l < r {
        let m = l + (r - l) / 2;
        let cell = page.cell(m)?;

        if is_table {
            let row_id = Value::Integer(cell.row_id() as _);

            if &row_id >= target {
                r = m;
            } else {
                l = m + 1;
            }
        } else {
            let entry = page.record_of(&cell, pager)?;
            let (ord, _full) = compare_index_entry(&entry, target)?;
            if ord == Ordering::Equal {
                saw_eq = true;
                r = m;
            } else if ord == Ordering::Greater {
                r = m;
            } else {
                l = m + 1;
            }
        }
    }

    if l < cell_count {
        return Ok(SearchResult::Descend {
            child: page.cell(l)?.left_child(),
            cell_index: l,
            exact: saw_eq,
        });
    }

    // Past the end: every key compared less-than, so no equality seen.
    Ok(SearchResult::Descend {
        child: page.right_most_ptr()?.unwrap(),
        cell_index: cell_count,
        exact: false,
    })
}

pub(crate) fn compare_index_entry(
    entry: &[Value],
    target: &Value,
) -> Result<(Ordering, bool), SqliteError> {
    let keys = match target {
        Value::Tuple(cols) => cols,
        _ => {
            return Err(SqliteError::Internal(
                "index seek target must be a tuple of key columns".into(),
            ));
        }
    };
    if keys.len() > entry.len() {
        return Err(SqliteError::Internal(format!(
            "index seek target has {} columns but entries hold {}",
            keys.len(),
            entry.len()
        )));
    }
    for (stored, wanted) in entry.iter().zip(keys.iter()) {
        if stored == wanted {
            continue;
        }
        if stored > wanted {
            return Ok((Ordering::Greater, false));
        }
        return Ok((Ordering::Less, false));
    }
    Ok((Ordering::Equal, keys.len() == entry.len()))
}

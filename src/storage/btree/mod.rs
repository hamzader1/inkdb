pub mod cursor;
pub mod delete;
pub mod insert;
pub mod rebalance;
pub mod split;

pub use cursor::{BTreeCursor, CursorState, Path, RestorePosition, SeekResult};

use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::cell::BTreeCell;

use super::page::PageMut as BTreePageMut;
pub use super::page::PageRef;
pub use super::page::PageRef as BTreePageRef;

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
        guard.bytes_as_ref(),
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
            guard.bytes_as_ref(),
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

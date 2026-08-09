use super::cell::BTreeCell;
use super::page::BTreePageRef;
use super::records::Value;
use crate::format::page;
use crate::pager::guard::PageGuard;
use crate::pager::pager::Pager;
use crate::storage::page::BTreePageType;
use crate::storage::records::SqlType;
use crate::vfs::file::SqliteFile;
use crate::PageNo;
use crate::SqliteError;

pub type CellIdx = u16;

#[derive(Debug, PartialEq)]
pub enum CursorState {
    At,
    Invalid,
    AfterLast,
    BeforeFirst,
}

#[derive(Debug, PartialEq)]
pub enum SeekResult {
    Exact,
    NotFound,
}

#[derive(Debug)]
pub struct Path {
    pub page_no: PageNo,
    pub cell_idx: u16,
    guard: PageGuard,
}
impl Path {
    fn new(page_no: PageNo, cell_idx: CellIdx, guard: PageGuard) -> Self {
        Self {
            page_no,
            cell_idx,
            guard,
        }
    }
}
#[derive(Debug)]
pub struct BTreeCursor {
    root: PageNo,
    pub stack: Vec<Path>,
    pub state: CursorState,
}
impl BTreeCursor {
    pub fn new(root: PageNo) -> Self {
        Self {
            root,
            stack: Vec::new(),
            state: CursorState::Invalid,
        }
    }
    pub fn seek<'a, P: SqliteFile>(
        &mut self,
        pager: &mut Pager<P>,
        target: Value<'a>,
    ) -> Result<SeekResult, SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = Self::page_as_ref(page_no, &guard, pager)?;
            if page.is_leaf() {
                let (found, cell_idx) = self.choose_target(&page, &target)?;
                self.stack.push(Path::new(page_no, cell_idx, guard));
                if found {
                    return Ok(SeekResult::Exact);
                }
                return Ok(SeekResult::NotFound);
            }
            self.state = CursorState::At;
            let (child, cell_idx) = self.choose_child(&page, &target)?;
            self.stack.push(Path::new(page_no, cell_idx, guard));
            page_no = child;
        }
    }

    pub fn next<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        while let Some(path) = self.stack.pop() {
            let Path {
                page_no,
                cell_idx,
                guard,
            } = path;

            let page = Self::page_as_ref(page_no, &guard, pager)?;
            if page.is_leaf() {
                if cell_idx + 1 < page.no_of_cells() {
                    self.stack.push(Path::new(page_no, cell_idx + 1, guard));
                    self.state = CursorState::At;
                    return Ok(());
                }
            } else if cell_idx + 1 == page.no_of_cells() {
                let child = page.right_most_ptr().ok_or(SqliteError::Corrupt(
                    "interior page has no right-most child".into(),
                ))?;
                self.add_path(page_no, cell_idx + 1, guard);
                self.descend_to_first(pager, child)?;
                self.state = CursorState::At;
                return Ok(());
            } else if cell_idx + 1 < page.no_of_cells() {
                let child = page.cell(cell_idx + 1)?.left_child();
                self.add_path(page_no, cell_idx + 1, guard);
                self.descend_to_first(pager, child)?;
                self.state = CursorState::At;
                return Ok(());
            }
        }
        self.state = CursorState::AfterLast;
        Ok(())
    }
    pub fn first<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = Self::page_as_ref(page_no, &guard, pager)?;
            if page.is_leaf() {
                self.add_path(page_no, 0, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.cell(0)?.left_child();
            self.add_path(page_no, 0, guard);
            page_no = child;
        }
    }
    pub fn descend_to_first<P: SqliteFile>(
        &mut self,
        pager: &mut Pager<P>,
        page_no: PageNo,
    ) -> Result<(), SqliteError> {
        let mut page_no = page_no;
        loop {
            let guard = pager.get(page_no)?;
            let page = Self::page_as_ref(page_no, &guard, pager)?;
            if page.is_leaf() {
                self.add_path(page_no, 0, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.cell(0)?.left_child();
            self.add_path(page_no, 0, guard);
            page_no = child;
        }
    }
    pub fn prev<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        while let Some(path) = self.stack.pop() {
            let Path {
                page_no,
                cell_idx,
                guard,
            } = path;
            let page = Self::page_as_ref(page_no, &guard, pager)?;
            if page.is_leaf() {
                if cell_idx > 0 {
                    self.add_path(page_no, cell_idx - 1, guard);
                    self.state = CursorState::At;
                    return Ok(());
                }
            } else {
                if cell_idx > 0 {
                    let child = page.cell(cell_idx - 1)?.left_child();
                    self.add_path(page_no, cell_idx - 1, guard);
                    self.descend_to_last(pager, child)?;
                    self.state = CursorState::At;
                    return Ok(());
                }
            }
        }
        self.state = CursorState::BeforeFirst;
        Ok(())
    }
    pub fn last<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = Self::page_as_ref(page_no, &guard, pager)?;
            if page.is_leaf() {
                self.add_path(page_no, page.no_of_cells() - 1, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.right_most_ptr().ok_or(SqliteError::Corrupt(
                "interior page has no right-most child".into(),
            ))?;
            self.add_path(page_no, page.no_of_cells(), guard);
            page_no = child;
        }
    }
    fn descend_to_last<P: SqliteFile>(
        &mut self,
        pager: &mut Pager<P>,
        page_no: PageNo,
    ) -> Result<(), SqliteError> {
        let mut page_no = page_no;
        loop {
            let guard = pager.get(page_no)?;
            let page = Self::page_as_ref(page_no, &guard, pager)?;
            if page.is_leaf() {
                self.add_path(page_no, page.no_of_cells() - 1, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.right_most_ptr().ok_or(SqliteError::Corrupt(
                "interior page has no right-most child".into(),
            ))?;
            self.add_path(page_no, page.no_of_cells(), guard);
            page_no = child;
        }
    }

    pub fn current<P: SqliteFile>(
        &self,
        pager: &mut Pager<P>,
    ) -> Result<Option<BTreeCell>, SqliteError> {
        if let Some(path) = self.stack.last() {
            let Path {
                page_no,
                cell_idx,
                guard,
            } = path;

            let page = Self::page_as_ref(*page_no, guard, pager)?;
            let cell = page.cell(*cell_idx)?;
            return Ok(Some(cell));
        }
        Ok(None)
    }
    fn clear_path(&mut self) {
        self.stack.clear();
    }
    fn choose_child<'a>(
        &self,
        page: &BTreePageRef<'a>,
        target: &Value,
    ) -> Result<(PageNo, CellIdx), SqliteError> {
        debug_assert!(
            page.is_interior(),
            "Navigation path of this works only with interior pages"
        );
        let cell_count = page.no_of_cells();
        if page.page_type() == BTreePageType::InteriorTable {
            let mut l = 0;
            let mut r = cell_count;
            while l < r {
                let m = l + (r - l) / 2;
                let cell = page.cell(m)?;
                let row_id = &cell.row_id().into_sqlite_value();
                if row_id >= target {
                    return Ok((cell.left_child(), m));
                } else if row_id > target {
                    r = m;
                } else {
                    l = m + 1
                }
            }
            return Ok((page.right_most_ptr().unwrap(), cell_count));
        }
        todo!("INDEX INTERIOR NOT IMPLEMENTED YET")
    }

    fn choose_target<'a>(
        &self,
        page: &BTreePageRef<'a>,
        target: &Value,
    ) -> Result<(bool, CellIdx), SqliteError> {
        assert!(page.is_leaf(), "This navigation path works only for leaves");
        let cell_cnt = page.no_of_cells();
        if page.page_type() == BTreePageType::LeafTable {
            let mut l = 0;
            let mut r = cell_cnt;
            while l < r {
                let m: u16 = l + ((r - l) / 2);
                let cell = page.cell(m)?;
                let row_id = &cell.row_id().into_sqlite_value();
                if row_id == target {
                    return Ok((true, m));
                } else if row_id > target {
                    r = m;
                } else {
                    l = m + 1;
                }
            }
            return Ok((false, l));
        }
        todo!("INDEX LEAF NOT IMPLEMENTED YET")
    }

    pub fn current_page_as_ref<'a, P: SqliteFile>(
        &'a self,
        pager: &mut Pager<P>,
    ) -> Result<Option<BTreePageRef<'a>>, SqliteError> {
        if let Some(path) = self.stack.last() {
            let page = Self::page_as_ref(path.page_no, &path.guard, pager)?;
            return Ok(Some(page));
        }
        Ok(None)
    }
    pub fn current_record<'a, P: SqliteFile>(
        &'a self,
        pager: &mut Pager<P>,
    ) -> Result<Option<Vec<Value<'a>>>, SqliteError> {
        if let Some(page) = self.current_page_as_ref(pager)? && let Some(cell) = self.current(pager)? {
            let cell = page.record_of(&cell, pager)?;
            return Ok(Some(cell));
        }
        Ok(None)
    }

    fn with_page<P: SqliteFile, T, F>(
        pager: &mut Pager<P>,
        page_no: PageNo,
        f: F,
    ) -> Result<T, SqliteError>
    where
        F: for<'a> FnOnce(&'a BTreePageRef<'a>) -> Result<T, SqliteError>,
    {
        let page_guard = pager.get(page_no)?;
        let page = BTreePageRef::new(
            page_no,
            page_guard.bytes_as_ref(),
            &page_guard,
            pager.metadata.page_size,
            pager.metadata.usable_size,
        )?;
        f(&page)
    }
    pub fn with_current<P, F, R>(&mut self, pager: &mut Pager<P>, f: F) -> Result<R, SqliteError>
    where
        P: SqliteFile,
        F: for<'a> FnOnce(&'a BTreePageRef<'a>, &BTreeCell) -> Result<R, SqliteError>,
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

    pub fn page_as_ref<'a, P: SqliteFile>(
        page_no: PageNo,
        guard: &'a PageGuard,
        pager: &Pager<P>,
    ) -> Result<BTreePageRef<'a>, SqliteError> {
        BTreePageRef::new(
            page_no,
            guard.bytes_as_ref(),
            guard,
            pager.metadata.page_size,
            pager.metadata.usable_size,
        )
    }

    fn add_path(&mut self, page_no: PageNo, cell_idx: CellIdx, guard: PageGuard) {
        self.stack.push(Path::new(page_no, cell_idx, guard));
    }
}

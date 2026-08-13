use super::cell::BTreeCell;
use super::page::BTreePageRef;
use crate::PageNo;
use crate::SqliteError;
use crate::format::page;
use crate::pager::guard::PageGuard;
use crate::pager::pager::Pager;
use crate::record::SqlType;
use crate::record::Value;
use crate::storage::page::BTreePageType;
use crate::util::sqlite_assert_with_corrupt_err;

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
enum SearchResult {
    Found { row_id: i64, cell_index: CellIdx },
    Descend { child: u32, cell_index: CellIdx },
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
    pub fn seek(
        &mut self,
        pager: &mut Pager,
        target: Value<'_>,
    ) -> Result<SeekResult, SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = Self::page_as_ref(page_no, &guard, pager)?;
            if page.is_leaf() {
                let (found, cell_idx) = self.choose_target(&page, pager, &target)?;
                self.stack.push(Path::new(page_no, cell_idx, guard));
                if found {
                    return Ok(SeekResult::Exact);
                }
                return Ok(SeekResult::NotFound);
            }
            self.state = CursorState::At;
            let search_result = self.choose_child(&page, pager, &target)?;
            match search_result {
                SearchResult::Found { row_id, cell_index } => {
                    self.stack.push(Path::new(page_no, cell_index, guard));
                    return Ok(SeekResult::Exact);
                }
                SearchResult::Descend { child, cell_index } => {
                    self.stack.push(Path::new(page_no, cell_index, guard));
                    page_no = child;
                }
            }
        }
    }

    pub fn next(&mut self, pager: &mut Pager) -> Result<(), SqliteError> {
        // TODO: Index cursor iteration requires visiting
        // interior index cells during traversal.
        //
        // Currently supported for table B-trees only.
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
            } else {
                if cell_idx + 1 == page.no_of_cells() {
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
        }
        self.state = CursorState::AfterLast;
        Ok(())
    }
    pub fn first(&mut self, pager: &mut Pager) -> Result<(), SqliteError> {
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
    pub fn descend_to_first(
        &mut self,
        pager: &mut Pager,
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
    pub fn prev(&mut self, pager: &mut Pager) -> Result<(), SqliteError> {
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
    pub fn last(&mut self, pager: &mut Pager) -> Result<(), SqliteError> {
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
    fn descend_to_last(&mut self, pager: &mut Pager, page_no: PageNo) -> Result<(), SqliteError> {
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

    pub fn current(&self, pager: &mut Pager) -> Result<Option<BTreeCell>, SqliteError> {
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
        pager: &mut Pager,
        target: &Value,
    ) -> Result<SearchResult, SqliteError> {
        sqlite_assert_with_corrupt_err(
            page.is_interior(),
            "Navigation path of this works only with interior pages",
        )?;
        let cell_count = page.no_of_cells();
        if page.page_type() == BTreePageType::InteriorTable {
            let mut l = 0;
            let mut r = cell_count;
            while l < r {
                let m = l + (r - l) / 2;
                let cell = page.cell(m)?;
                let row_id = &cell.row_id().into_sqlite_value();
                if row_id >= target {
                    r = m;
                } else {
                    l = m + 1
                }
            }
            if l < cell_count {
                let res = SearchResult::Descend {
                    child: page.cell(l)?.left_child(),
                    cell_index: l,
                };
                return Ok(res);
            }
        } else {
            let mut l = 0;
            let mut r = cell_count;
            while l < r {
                let m = l + (r - l) / 2;
                let cell = page.cell(m)?;
                let mut payload = page.record_of(&cell, pager)?;
                let row_id = payload.pop().unwrap().get_int()?;
                let tuple = Value::Tuple(payload);
                if &tuple == target {
                    return Ok(SearchResult::Found {
                        row_id,
                        cell_index: m,
                    });
                }
                if &tuple > target {
                    r = m;
                } else {
                    l = m + 1;
                }
            }
            if l < cell_count {
                return Ok(SearchResult::Descend {
                    child: page.cell(l)?.left_child(),
                    cell_index: l,
                });
            }
        }
        Ok(SearchResult::Descend {
            child: page.right_most_ptr().unwrap(),
            cell_index: cell_count,
        })
    }

    fn choose_target<'a>(
        &'a self,
        page: &BTreePageRef<'a>,
        pager: &mut Pager,
        target: &Value<'_>,
    ) -> Result<(bool, CellIdx), SqliteError> {
        sqlite_assert_with_corrupt_err(
            page.is_leaf(),
            "This navigation path works only for leaves",
        )?;
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
            Ok((false, l))
        } else {
            let mut l = 0;
            let mut r = cell_cnt;
            while l < r {
                let m: u16 = l + ((r - l) / 2);
                let mut payload = page.record_of_cell(m, pager)?;
                let row_id = payload.pop().unwrap().get_int()?;
                let tuple = Value::Tuple(payload);
                if &tuple == target {
                    return Ok((true, m));
                } else if &tuple > target {
                    r = m;
                } else {
                    l = m + 1;
                }
            }
            Ok((false, l))
        }
    }

    pub fn current_page_as_ref<'a>(
        &'a self,
        pager: &mut Pager,
    ) -> Result<Option<BTreePageRef<'a>>, SqliteError> {
        if let Some(path) = self.stack.last() {
            let page = Self::page_as_ref(path.page_no, &path.guard, pager)?;
            return Ok(Some(page));
        }
        Ok(None)
    }
    pub fn current_record<'a>(
        &'a self,
        pager: &mut Pager,
    ) -> Result<Option<Vec<Value<'a>>>, SqliteError> {
        if let Some(page) = self.current_page_as_ref(pager)?
            && let Some(cell) = self.current(pager)?
        {
            let cell = page.record_of(&cell, pager)?;
            return Ok(Some(cell));
        }
        Ok(None)
    }

    fn with_page<T, FN>(pager: &mut Pager, page_no: PageNo, f: FN) -> Result<T, SqliteError>
    where
        FN: for<'a> FnOnce(&'a BTreePageRef<'a>) -> Result<T, SqliteError>,
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
    pub fn with_current<FN, R>(&mut self, pager: &mut Pager, f: FN) -> Result<R, SqliteError>
    where
        FN: for<'a> FnOnce(&'a BTreePageRef<'a>, &BTreeCell) -> Result<R, SqliteError>,
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

    pub fn page_as_ref<'a>(
        page_no: PageNo,
        guard: &'a PageGuard,
        pager: &Pager,
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

use super::cell::BTreeCell;
use super::cell::Encode;
use super::freelist::FreeList;
use super::page::BTreePageMut;
use super::page::BTreePageOps;
use super::page::BTreePageRef;
use super::page::InsertionState;
use super::page::PageField::*;
use std::fmt::Debug;

use crate::SqliteResult;
use crate::pager::pager::PageNo;

use crate::SqliteCursor;
use crate::SqliteError;

use crate::pager::guard::PageGuard;
use crate::pager::pager::Pager;
use crate::record::SqlType;
use crate::record::Value;
use crate::storage::cell::TableInteriorCell;
use crate::storage::page::BTreePageType;
use crate::storage::page::compute_table_local_payload_size;
use crate::util::sqlite_assert_with_corrupt_err;

pub const DATABASE_SIZE_IN_PAGES_OFFSET: usize = 28;
pub const DATABASE_SIZE_IN_PAGES_SIZE: usize = 4;
pub type CellIndex = u16;

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
enum UnderflowAction {
    BorrowLeft,
    BorrowRight,
    Both,
}

#[derive(Debug)]
pub struct Path {
    pub page_no: PageNo,
    pub cell_idx: u16,
    guard: PageGuard,
}
impl Path {
    fn new(page_no: PageNo, cell_idx: CellIndex, guard: PageGuard) -> Self {
        Self {
            page_no,
            cell_idx,
            guard,
        }
    }
}
pub enum SearchResult {
    Found { row_id: i64, cell_index: CellIndex },
    Descend { child: u32, cell_index: CellIndex },
}
impl SearchResult {
    pub fn cell_index(&self) -> CellIndex {
        match self {
            Self::Found { cell_index, .. } => *cell_index,
            Self::Descend { cell_index, .. } => *cell_index,
        }
    }
}

#[derive(Debug)]
pub struct BTreeCursor<F: crate::vfs::file::SqliteFile> {
    root: PageNo,
    pub stack: Vec<Path>,
    pub state: CursorState,
    _phantom: std::marker::PhantomData<F>,
}
impl<F: crate::vfs::file::SqliteFile> BTreeCursor<F> {
    pub fn new(root: PageNo) -> Self {
        Self {
            root,
            stack: Vec::new(),
            state: CursorState::Invalid,
            _phantom: std::marker::PhantomData,
        }
    }
    pub fn seek(
        &mut self,
        pager: &mut Pager<F>,
        target: Value<'_>,
    ) -> Result<SeekResult, SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf() {
                let (found, cell_idx) = self.binary_search_leaf(&page, pager, &target)?;
                self.stack.push(Path::new(page_no, cell_idx, guard));
                if found {
                    return Ok(SeekResult::Exact);
                }
                return Ok(SeekResult::NotFound);
            }
            self.state = CursorState::At;
            let search_result = self.binary_search_interior(&page, pager, &target)?;
            match search_result {
                SearchResult::Found { cell_index, .. } => {
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

    pub fn next(&mut self, pager: &mut Pager<F>) -> Result<(), SqliteError> {
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

            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf() {
                if cell_idx + 1 < page.no_of_cells() {
                    self.stack.push(Path::new(page_no, cell_idx + 1, guard));
                    self.state = CursorState::At;
                    return Ok(());
                }
            } else {
                if cell_idx + 1 == page.no_of_cells() {
                    let child = page.right_most_ptr().ok_or(SqliteError::Internal(format!(
                        "cursor next: interior page {page_no} has no right-most child"
                    )))?;
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
    pub fn first(&mut self, pager: &mut Pager<F>) -> Result<(), SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
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
        pager: &mut Pager<F>,
        page_no: PageNo,
    ) -> Result<(), SqliteError> {
        let mut page_no = page_no;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
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
    pub fn prev(&mut self, pager: &mut Pager<F>) -> Result<(), SqliteError> {
        while let Some(path) = self.stack.pop() {
            let Path {
                page_no,
                cell_idx,
                guard,
            } = path;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
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
    pub fn last(&mut self, pager: &mut Pager<F>) -> Result<(), SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf() {
                let cell_idx = if page.no_of_cells() == 0 {
                    0
                } else {
                    page.no_of_cells() - 1
                };
                self.add_path(page_no, cell_idx, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.right_most_ptr().ok_or(SqliteError::Internal(format!(
                "cursor last: interior page {page_no} has no right-most child"
            )))?;
            self.add_path(page_no, page.no_of_cells(), guard);
            page_no = child;
        }
    }
    fn descend_to_last(
        &mut self,
        pager: &mut Pager<F>,
        page_no: PageNo,
    ) -> Result<(), SqliteError> {
        let mut page_no = page_no;
        loop {
            let guard = pager.get(page_no)?;
            let page = page_as_ref_with_pager(page_no, &guard, pager)?;
            if page.is_leaf() {
                self.add_path(page_no, page.no_of_cells() - 1, guard);
                self.state = CursorState::At;
                return Ok(());
            }
            let child = page.right_most_ptr().ok_or(SqliteError::Internal(format!(
                "cursor descend_to_last: interior page {page_no} has no right-most child"
            )))?;
            self.add_path(page_no, page.no_of_cells(), guard);
            page_no = child;
        }
    }

    pub fn current(&self, pager: &mut Pager<F>) -> Result<Option<BTreeCell>, SqliteError> {
        if let Some(path) = self.stack.last() {
            let Path {
                page_no,
                cell_idx,
                guard,
            } = path;

            let page = page_as_ref_with_pager(*page_no, guard, pager)?;
            let cell = page.cell(*cell_idx)?;
            return Ok(Some(cell));
        }
        Ok(None)
    }
    fn clear_path(&mut self) {
        self.stack.clear();
    }
    fn binary_search_interior<'g, P>(
        &self,
        page: &'g P,
        pager: &mut Pager<F>,
        target: &Value,
    ) -> Result<SearchResult, SqliteError>
    where
        P: BTreePageOps<'g>,
    {
        sqlite_assert_with_corrupt_err(
            page.is_interior(),
            "Navigation path of this works only with interior pages",
        )?;

        let cell_count = page.no_of_cells();
        let is_table = page.page_type() == BTreePageType::InteriorTable;

        let mut l = 0;
        let mut r = cell_count;

        while l < r {
            let m = l + (r - l) / 2;
            let cell = page.cell(m)?;

            if is_table {
                let row_id = cell.row_id().into_sqlite_value();

                if &row_id >= target {
                    r = m;
                } else {
                    l = m + 1;
                }
            } else {
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
        }

        if l < cell_count {
            return Ok(SearchResult::Descend {
                child: page.cell(l)?.left_child(),
                cell_index: l,
            });
        }

        Ok(SearchResult::Descend {
            child: page.right_most_ptr().unwrap(),
            cell_index: cell_count,
        })
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

    fn binary_search_leaf<'a, P>(
        &self,
        page: &'a P,
        pager: &mut Pager<F>,
        target: &Value<'_>,
    ) -> Result<(bool, CellIndex), SqliteError>
    where
        P: BTreePageOps<'a> + Debug,
    {
        sqlite_assert_with_corrupt_err(
            page.is_leaf(),
            "This navigation path works only for leaves",
        )?;

        let cell_cnt = page.no_of_cells();
        let mut l = 0;
        let mut r = cell_cnt;

        while l < r {
            let m: u16 = l + ((r - l) / 2);

            let value = if page.page_type() == BTreePageType::LeafTable {
                page.cell(m)?.row_id().into_sqlite_value()
            } else {
                Value::Tuple(page.record_of_cell(m, pager)?)
            };

            if &value == target {
                return Ok((true, m));
            } else if &value > target {
                r = m;
            } else {
                l = m + 1;
            }
        }

        Ok((false, l))
    }

    pub fn current_page_as_ref<'a>(
        &'a self,
        pager: &mut Pager<F>,
    ) -> Result<Option<BTreePageRef<'a>>, SqliteError> {
        if let Some(path) = self.stack.last() {
            let page = page_as_ref_with_pager(path.page_no, &path.guard, pager)?;
            return Ok(Some(page));
        }
        Ok(None)
    }
    pub fn current_record<'a>(
        &'a self,
        pager: &mut Pager<F>,
    ) -> Result<Option<Vec<Value<'a>>>, SqliteError> {
        if let Some(page) = self.current_page_as_ref(pager)?
            && let Some(cell) = self.current(pager)?
        {
            let cell = page.record_of(&cell, pager)?;
            return Ok(Some(cell));
        }
        Ok(None)
    }

    fn with_page<T, FN>(pager: &mut Pager<F>, page_no: PageNo, f: FN) -> Result<T, SqliteError>
    where
        FN: for<'a> FnOnce(&'a BTreePageRef<'a>) -> Result<T, SqliteError>,
    {
        let page_guard = pager.get(page_no)?;
        let page = BTreePageRef::new(
            page_no,
            page_guard.bytes_as_ref(),
            pager.metadata.page_size,
            pager.metadata.usable_size,
        )?;
        f(&page)
    }
    pub fn with_current<FN, R>(&mut self, pager: &mut Pager<F>, f: FN) -> Result<R, SqliteError>
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

pub struct BTree<'a, F: crate::vfs::file::SqliteFile> {
    pub root_page: PageNo,
    pager: &'a mut Pager<F>,
    pub cursor: BTreeCursor<F>,
}

#[derive(Debug, Clone)]
pub struct SplitMetadata {
    pub left_page: u32,
    pub right_page: u32,
    pub boundary: Value<'static>,
    pub right_max: Value<'static>,
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
        }
    }
}
impl<'a, F: crate::vfs::file::SqliteFile> BTree<'a, F> {
    pub fn new(root_page: PageNo, pager: &'a mut Pager<F>) -> Self {
        Self {
            root_page,
            pager,
            cursor: BTreeCursor::new(root_page),
        }
    }
    pub fn with_cursor(pager: &'a mut Pager<F>, cursor: BTreeCursor<F>) -> Self {
        Self {
            root_page: cursor.root,
            pager,
            cursor,
        }
    }

    pub fn insert(&mut self, key: Value, mut content: Vec<u8>) -> Result<(), SqliteError> {
        self.cursor.seek(self.pager, key.into_owned())?;
        let (page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();
        let mut page_guard = self.pager.get_mut(page_no)?;
        let mut page = self.page_as_mut(page_no, &mut page_guard)?;
        self.fix_overlow(&mut content)?;
        if let InsertionState::Inserted = page.insert_cell(&content, cell_idx)? {
            return Ok(());
        } else {
            let meta = self.balance(page_no)?;
            self.insert_key_to_leaf(&key, content, meta)?;
        }
        Ok(())
    }

    pub fn fix_overlow(&mut self, content: &mut Vec<u8>) -> Result<(), SqliteError> {
        let usable_size = self.pager.metadata.usable_size;
        if content.len() <= self.pager.metadata.usable_size {
            return Ok(());
        }
        // TODO TEMPORARY FOR TABLE BTREE ONLY
        let local_payload_len = compute_table_local_payload_size(usable_size, content.len());
        let overflow_data = content.split_off(local_payload_len);
        let first_overflow_page = self.allocate_page()?;
        content.extend_from_slice(&u32::to_be_bytes(first_overflow_page));

        let mut cursor = SqliteCursor::new(&overflow_data);
        let mut curr_page = first_overflow_page;
        let mut remaining = overflow_data.len();
        while remaining > 0 {
            let mut guard = self.pager.get_mut(curr_page)?;
            let page_bytes = guard.bytes_as_mut().unwrap();
            let bytes_to_write = remaining.min(usable_size - 4);
            let slice = &mut page_bytes[..usable_size];
            cursor.read_next_exact(&mut slice[4..4 + bytes_to_write])?;
            remaining -= bytes_to_write;
            if remaining == 0 {
                curr_page = 0;
            } else {
                curr_page = self.allocate_page()?;
            }
            slice[0..4].copy_from_slice(&u32::to_be_bytes(curr_page));
        }
        Ok(())
    }

    pub fn balance(&mut self, page_no: PageNo) -> Result<SplitMetadata, SqliteError> {
        let split_metadata = self.split_leaf(page_no)?; // THE TWO LEAVES WE WANT TO RETURN

        self.cursor.stack.pop(); // WE POP LEAF, WE ARE AT PARENT
        // LEFT PAGE
        let mut left_page_guard = self.pager.get_mut(split_metadata.left_page)?;
        let mut left_page = self.page_as_mut(split_metadata.left_page, &mut left_page_guard)?;
        // RIGHT PAGE
        let mut right_page_guard = self.pager.get_mut(split_metadata.right_page)?;
        let right_page = self.page_as_mut(split_metadata.right_page, &mut right_page_guard)?;

        if let Some(path) = self.cursor.stack.pop() {
            let parent_page_as_ref = self.page_as_ref(path.page_no, &path.guard)?;
            let index = self
                .cursor
                .binary_search_interior(&parent_page_as_ref, self.pager, &split_metadata.boundary)?
                .cell_index();
            let left_page_payload = Encode::encode_table_interior_cell(
                left_page.page_no,
                split_metadata.boundary.get_int()? as _,
            );
            let right_page_payload = Encode::encode_table_interior_cell(
                right_page.page_no,
                split_metadata.right_max.get_int()? as _,
            );

            let mut guard = self.pager.get_mut(path.page_no)?;
            let mut parent_page_as_mut = self.page_as_mut(path.page_no, &mut guard)?;

            let was_rightmost =
                parent_page_as_mut.header.right_most_ptr() == Some(split_metadata.left_page);

            if was_rightmost {
                // the divider becomes the parent's new last cell and the
                // right-most pointer is re-pointed at the new right page
                match parent_page_as_mut.insert_cell(&left_page_payload, index)? {
                    InsertionState::Inserted => {
                        parent_page_as_mut.header.right_most_ptr = Some(split_metadata.right_page);

                        parent_page_as_mut.update_bytes([RightMostPointer]);
                        Ok(split_metadata)
                    }
                    InsertionState::None => {
                        let meta = self.split_interior(parent_page_as_mut.page_no)?;
                        let key = split_metadata.boundary.into_owned();
                        self.insert_key_to_interior(&key, left_page_payload, meta.clone())?;
                        let mut guard = self.pager.get_mut(meta.right_page)?;
                        let mut page = self.page_as_mut(meta.right_page, &mut guard)?;
                        page.header.right_most_ptr = Some(split_metadata.right_page);
                        page.update_bytes([RightMostPointer]);
                        Ok(split_metadata)
                    }
                }
            } else {
                // the old cell already points at the left page; only its key
                // becomes the boundary, then a divider for the right page follows
                //
                // TODO: optimaze left cell insertion from rebuild to in place insert
                parent_page_as_mut.replace_cell(index, &left_page_payload)?;
                match parent_page_as_mut.insert_cell(&right_page_payload, index + 1)? {
                    InsertionState::Inserted => Ok(split_metadata),
                    InsertionState::None => {
                        let meta = self.split_interior(parent_page_as_mut.page_no)?;
                        let key = split_metadata.right_max.into_owned();
                        self.insert_key_to_interior(&key, right_page_payload, meta)?;
                        Ok(split_metadata)
                    }
                }
            }
        } else {
            let new_left_page_no = self.allocate_page()?;
            let mut new_left_page_guard = self.pager.get_mut(new_left_page_no)?;
            let mut new_left_page = BTreePageMut::new_from_raw_bytes(
                new_left_page_no,
                BTreePageType::LeafTable,
                new_left_page_guard.bytes_as_mut().unwrap(),
                self.pager.metadata.page_size,
                self.pager.metadata.usable_size,
            );
            new_left_page.copy_data_from(&left_page)?;
            // TODO REMOVE THIS LATER IF WE DONE FROM IT:
            left_page.clear();
            //
            // rebuild metadata
            let left_last_ptr = new_left_page.cell_pointers.last().copied().ok_or_else(|| {
                SqliteError::Internal("root split with an empty left leaf".into())
            })?;
            let rowid = new_left_page.parse_cell_at(left_last_ptr)?.row_id();
            let right_last_ptr = right_page.cell_pointers.last().copied().ok_or_else(|| {
                SqliteError::Internal("root split with an empty right leaf".into())
            })?;
            let right_max = right_page.parse_cell_at(right_last_ptr)?.row_id();

            let mut root = BTreePageMut::new_from_raw_bytes(
                left_page.page_no,
                BTreePageType::InteriorTable,
                left_page_guard.bytes_as_mut().unwrap(),
                self.pager.metadata.page_size,
                self.pager.metadata.usable_size,
            );

            root.header.right_most_ptr = Some(right_page.page_no);
            root.update_bytes([RightMostPointer]);

            let left_child_payload =
                Encode::encode_table_interior_cell(new_left_page_no, rowid as _);
            root.insert_cell(&left_child_payload, 0)?;
            Ok(SplitMetadata::new(
                new_left_page_no,
                right_page.page_no,
                rowid.into_sqlite_value(),
                right_max.into_sqlite_value(),
            ))
        }
    }

    pub fn split_leaf(&mut self, page_no: PageNo) -> Result<SplitMetadata, SqliteError> {
        let mut left_page_guard = self.pager.get_mut(page_no)?;
        let mut left_page = self.page_as_mut(page_no, &mut left_page_guard)?;

        debug_assert!(
            left_page.cell_pointers.len() >= 2,
            "cannot split a leaf page holding fewer than two cells",
        );
        // TODO add freelist check
        let right_page_no = self.allocate_page()?;
        let mut right_page_guard = self.pager.get_mut(right_page_no)?;
        let mut right_page = BTreePageMut::new_from_raw_bytes(
            right_page_no,
            BTreePageType::LeafTable,
            right_page_guard.bytes_as_mut().unwrap(),
            self.pager.metadata.page_size,
            self.pager.metadata.usable_size,
        );
        let split_at = left_page.cell_pointers.len() / 2;
        let right_cell_pointers = left_page.cell_pointers.split_off(split_at);
        let mut left_cells: Vec<Vec<u8>> = Vec::with_capacity(left_page.cell_pointers.len());
        for i in 0..left_page.cell_pointers.len() {
            let span = left_page.cell_span(left_page.cell_pointers[i])?;
            left_cells.push(left_page.bytes[span].to_vec());
        }
        let mut right_cells: Vec<&[u8]> = Vec::with_capacity(right_cell_pointers.len());
        for &cell_offset in right_cell_pointers.iter() {
            let span = left_page.cell_span(cell_offset)?;
            right_cells.push(&left_page.bytes[span]);
        }

        for (i, cell) in right_cells.iter().enumerate() {
            if right_page.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "right leaf page overflowed during split".into(),
                ));
            }
        }

        /*
         * UPDATE INCLUDE:
         *
         *  CELL POINTERS
         *  CELL COUNT
         *  CELL CONTENT AREA
         *
         */
        left_page.reset_for_rebuild();
        for (i, cell) in left_cells.iter().enumerate() {
            if left_page.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "left leaf page overflowed during split".into(),
                ));
            }
        }

        let left_last_ptr =
            left_page.cell_pointers.last().copied().ok_or_else(|| {
                SqliteError::Internal("left leaf page is empty after split".into())
            })?;
        let right_last_ptr =
            right_page.cell_pointers.last().copied().ok_or_else(|| {
                SqliteError::Internal("right leaf page is empty after split".into())
            })?;
        let metadata = SplitMetadata::new(
            left_page.page_no,
            right_page.page_no,
            left_page
                .parse_cell_at(left_last_ptr)?
                .row_id()
                .into_sqlite_value(),
            right_page
                .parse_cell_at(right_last_ptr)?
                .row_id()
                .into_sqlite_value(),
        );

        Ok(metadata)
    }

    pub fn split_interior(&mut self, page_no: PageNo) -> Result<SplitMetadata, SqliteError> {
        // ORIGINAL PAGE
        let mut interior_page_guard = self.pager.get_mut(page_no)?;
        let mut interior_page = self.page_as_mut(page_no, &mut interior_page_guard)?;
        // one cell would make the pop below panic, two would leave the new page
        // without a single cell
        debug_assert!(
            interior_page.cell_pointers.len() >= 3,
            "cannot split an interior page holding fewer than three cells",
        );

        // TO BE LEFT
        let new_page_no = self.allocate_page()?;
        let mut new_page_guard = self.pager.get_mut(new_page_no)?;
        // let mut new_page = self.page_as_mut(new_page_no, &mut new_page_guard)?;
        let mut new_page = BTreePageMut::new_from_raw_bytes(
            new_page_no,
            BTreePageType::InteriorTable,
            new_page_guard.bytes_as_mut().unwrap(),
            self.pager.metadata.page_size,
            self.pager.metadata.usable_size,
        );

        let mut left_cell_pointers = std::mem::take(&mut interior_page.cell_pointers);
        let right_cell_pointers = left_cell_pointers.split_off(left_cell_pointers.len() / 2);
        let promoted_cell_offset = left_cell_pointers
            .pop()
            .ok_or_else(|| SqliteError::Internal("interior split left half is empty".into()))?;
        let cell_to_be_promoted = interior_page.parse_cell_at(promoted_cell_offset)?;

        // stage both halves before writing anything, both are read from the
        // bytes of the original page
        let mut left_cells: Vec<&[u8]> = Vec::with_capacity(left_cell_pointers.len());
        for &cell_offset in left_cell_pointers.iter() {
            let span = interior_page.cell_span(cell_offset)?;
            left_cells.push(&interior_page.bytes[span]);
        }
        let mut right_cells: Vec<Vec<u8>> = Vec::with_capacity(right_cell_pointers.len());
        for &cell_offset in right_cell_pointers.iter() {
            let span = interior_page.cell_span(cell_offset)?;
            right_cells.push(interior_page.bytes[span].to_vec());
        }

        for (i, cell) in left_cells.iter().enumerate() {
            if new_page.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "new interior page overflowed during split".into(),
                ));
            }
        }
        new_page.header.right_most_ptr = Some(cell_to_be_promoted.left_child());

        new_page.update_bytes([RightMostPointer]);

        interior_page.reset_for_rebuild();
        for (i, cell) in right_cells.iter().enumerate() {
            if interior_page.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "interior page overflowed during split".into(),
                ));
            }
        }
        // PROMOTE KEY STAGE

        let promoted_cell_payload =
            Encode::encode_table_interior_cell(new_page.page_no, cell_to_be_promoted.row_id() as _);
        let promoted_key = cell_to_be_promoted.row_id().into_sqlite_value();

        if let Some(path) = self.cursor.stack.pop() {
            let mut parent_guard = self.pager.get_mut(path.page_no)?;
            let mut parent_page = self.page_as_mut(path.page_no, &mut parent_guard)?;

            let page_as_ref = parent_page.as_ref()?;
            let cell_idx = self
                .cursor
                .binary_search_interior(&page_as_ref, self.pager, &promoted_key)?
                .cell_index();
            match parent_page.insert_cell(&promoted_cell_payload, cell_idx)? {
                InsertionState::Inserted => Ok(SplitMetadata::new(
                    new_page_no,
                    interior_page.page_no,
                    promoted_key,
                    cell_to_be_promoted.row_id().into_sqlite_value(),
                )),
                InsertionState::None => {
                    let split_metadata = self.split_interior(path.page_no)?;
                    let key = promoted_key;
                    self.insert_key_to_interior(&key, promoted_cell_payload, split_metadata)?;
                    Ok(SplitMetadata::new(
                        new_page_no,
                        interior_page.page_no,
                        key,
                        cell_to_be_promoted.row_id().into_sqlite_value(),
                    ))
                }
            }
        } else {
            // We are the root
            //
            let new_right_page_no = self.allocate_page()?;
            let mut new_right_page_guard = self.pager.get_mut(new_right_page_no)?;
            let mut new_right_page = BTreePageMut::new_from_raw_bytes(
                new_right_page_no,
                BTreePageType::InteriorTable,
                new_right_page_guard.bytes_as_mut().unwrap(),
                self.pager.metadata.page_size,
                self.pager.metadata.usable_size,
            );

            new_right_page.copy_data_from(&interior_page)?;
            interior_page.clear();

            // SAFE TO USE THE METADATA SINCE ITS CACHED
            let mut root = BTreePageMut::new_from_raw_bytes(
                interior_page.page_no,
                BTreePageType::InteriorTable,
                interior_page_guard.bytes_as_mut().unwrap(),
                self.pager.metadata.page_size,
                self.pager.metadata.usable_size,
            );

            root.header.right_most_ptr = Some(new_right_page_no);
            root.update_bytes([RightMostPointer]);

            root.insert_cell(&promoted_cell_payload, 0)?;

            Ok(SplitMetadata::new(
                new_page_no,
                new_right_page_no,
                cell_to_be_promoted.row_id().into_sqlite_value(),
                cell_to_be_promoted.row_id().into_sqlite_value(),
            ))
        }
    }

    pub fn insert_key_to_interior<T: AsRef<[u8]>>(
        &mut self,
        key: &Value,
        payload: T,
        meta: SplitMetadata,
    ) -> Result<(), SqliteError> {
        let target_page = if *key <= meta.boundary {
            meta.left_page
        } else {
            meta.right_page
        };
        let mut page_guard = self.pager.get_mut(target_page)?;
        let mut page_mut = self.page_as_mut(target_page, &mut page_guard)?;
        let cell_idx = self
            .cursor
            .binary_search_interior(&page_mut, self.pager, key)?
            .cell_index();
        page_mut.insert_cell(&payload, cell_idx)?;
        Ok(())
    }
    pub fn insert_key_to_leaf<T: AsRef<[u8]>>(
        &mut self,
        key: &Value,
        payload: T,
        meta: SplitMetadata,
    ) -> Result<(), SqliteError> {
        let target_page = if *key <= meta.boundary {
            meta.left_page
        } else {
            meta.right_page
        };
        let mut page_guard = self.pager.get_mut(target_page)?;
        let mut page_mut = self.page_as_mut(target_page, &mut page_guard)?;
        let (_, cell_idx) = self.cursor.binary_search_leaf(&page_mut, self.pager, key)?;
        page_mut.insert_cell(&payload, cell_idx)?;
        Ok(())
    }
    pub fn page_as_ref(
        &self,
        page_no: PageNo,
        guard: &'a PageGuard,
    ) -> Result<BTreePageRef<'a>, SqliteError> {
        BTreePageRef::new(
            page_no,
            guard.bytes_as_ref(),
            self.pager.metadata.page_size,
            self.pager.metadata.usable_size,
        )
    }

    pub fn page_as_mut(
        &self,
        page_no: PageNo,
        guard: &'a mut PageGuard,
    ) -> Result<BTreePageMut<'a>, SqliteError> {
        BTreePageMut::new(
            page_no,
            guard.bytes_as_mut().unwrap(),
            self.pager.metadata.page_size,
            self.pager.metadata.usable_size,
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

    // TODO:
    //     USE FREE LIST AS PRIMARY SOURCE, THEN ALLOCATE IF NONE
    //
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
    pub fn current_page_header_unchecked(
        &mut self,
    ) -> Result<super::page::BTreePageHeader, SqliteError> {
        let (pn, _) = self.cursor.last_visited_entry_unchecked();

        let header =
            self.with_page_ref::<_, super::page::BTreePageHeader>(pn, |page| Ok(page.header()))?;
        Ok(header)
    }

    // delete
    //
    pub fn delete(&mut self, key: Value) -> SqliteResult<()> {
        self.cursor.seek(self.pager, key.clone())?;
        let (page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();
        let found_key = self
            .with_page_ref(page_no, |page| {
                let key = page.cell_key(cell_idx)?;
                Ok(Some(key))
            })?
            .unwrap();
        if !(found_key.into_sqlite_value() == key) {
            // key not found
            return Ok(());
        }
        let (page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();
        // println!("###\nDelete initiale path:");
        // println!("PageN: {}:", page_no);
        // println!("CellId: {}\n###", cell_idx);

        let is_underflow = self.with_page_mut::<_, bool>(page_no, |page| {
            // println!("BEFORE CALLING REMOVE CELL");
            // dbg!(&page);
            page.remove_cell(cell_idx)?;
            let is_undeflow = page.is_underflow()?;
            Ok(is_undeflow)
        })?;
        if page_no == self.root_page {
            return Ok(());
        }
        if is_underflow {
            self.fix_page_underflow(page_no)?;
        }

        Ok(())
    }
    /// Collapse an empty interior root: move its single (right-most) child
    /// into the root page, keeping the root page_no stable so the catalog
    /// stays valid. Leaf roots and roots with >=1 key are left alone.
    /// The orphaned child page is leaked for now (TODO: freelist).
    fn collapse_root(&mut self, root_no: PageNo) -> SqliteResult<()> {
        let (n_cells, is_leaf, rmp) = self.with_page_ref(root_no, |page| {
            Ok((page.no_of_cells(), page.is_leaf(), page.right_most_ptr()))
        })?;
        if is_leaf || n_cells > 0 {
            return Ok(());
        }
        let child_no = rmp.ok_or(SqliteError::Internal(
            "empty interior root has no right-most child".into(),
        ))?;
        // Collect the surviving child before overwriting the root.
        let (kind, child_rmp, cells) = self.with_page_mut(child_no, |child| {
            let mut cells = Vec::with_capacity(child.no_of_cells() as usize);
            for i in 0..child.no_of_cells() {
                cells.push(child.cell_bytes_as_ref(i)?.to_vec());
            }
            Ok((child.header.page_kind, child.header.right_most_ptr, cells))
        })?;
        self.with_page_mut(root_no, |root| {
            root.reset_for_rebuild();
            root.header.page_kind = kind;
            root.header.right_most_ptr = child_rmp;
            root.update_bytes([PageKind, RightMostPointer]);
            for (i, bytes) in cells.iter().enumerate() {
                if root.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "root collapse: child cells do not fit in root".into(),
                    ));
                }
            }
            Ok(())
        })?;
        // The child's content now lives in the root; free the orphan.
        self.deallocate_page(child_no)?;
        Ok(())
    }
    /*
     * THIS FUNCTION RELIES ON THE UNDERFLOW PAGE BEING THE LAST ENTRY
     * IN THE PATH. WE MUST ENSURE THE PATH IS POSITIONED
     * AT THE PAGE CURRENTLY BEING REPAIRED.
     */
    pub fn fix_page_underflow(&mut self, child_page_no: PageNo) -> SqliteResult<()> {
        /*
         * TO FIX UNDERFLOW ON A PAGE
         * WE REQUIRE AT LEAST THE PAGE IT SELF AND ITS PARENT
         */
        if self.cursor.stack.is_empty() {
            return Ok(());
        }
        let _page_no = self.cursor.stack.pop().unwrap().page_no;

        debug_assert_eq!(
            _page_no, child_page_no,
            "The given page ({}) does not match the last page in the path ({})",
            _page_no, child_page_no
        );

        if self.cursor.stack.is_empty() {
            // the popped page was the root.
            // Roots don't underflow: a leaf root with 0 cells is an empty
            // table, an interior root with >=1 key is fine. Only an interior
            // root with 0 keys collapses (its RMP child moves into the root,
            // keeping the root page_no stable so the catalog stays valid).
            if _page_no != self.root_page {
                return Err(SqliteError::Internal(
                    "underflow path popped a non-root page with empty stack".into(),
                ));
            }
            self.collapse_root(_page_no)?;
            return Ok(());
        }

        let (parent_page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();
        let parent_n = self.with_page_ref(parent_page_no, |page| Ok(page.no_of_cells()))?;
        if parent_n == 0 {
            if parent_page_no != self.root_page {
                return Err(SqliteError::Internal(
                    "non-root interior page with 0 cells".into(),
                ));
            }
            self.collapse_root(parent_page_no)?;
            return Ok(());
        }
        let undeflow_action = self.underflow_planner(cell_idx, parent_n);
        let path = ActivePath::from(self.cursor.stack.as_ref());
        self.try_fix_underflow(undeflow_action, child_page_no, path)?;
        // println!("Underflow Fixed on pageno {}", child_page_no);
        Ok(())
    }
    fn underflow_planner(&self, cell_idx: CellIndex, parent_cells: u16) -> UnderflowAction {
        if cell_idx == 0 {
            UnderflowAction::BorrowRight
        } else if cell_idx == parent_cells {
            UnderflowAction::BorrowLeft
        } else {
            UnderflowAction::Both
        }
    }
    fn try_fix_underflow(
        &mut self,
        underflow_action: UnderflowAction,
        child_page_no: PageNo,
        parent_path: ActivePath,
    ) -> SqliteResult<()> {
        // dbg!(&parent_path, child_page_no, &underflow_action);
        match underflow_action {
            UnderflowAction::BorrowLeft => self.try_borrow_left_v2(child_page_no, parent_path)?,
            UnderflowAction::BorrowRight => self.try_borrow_right_v2(child_page_no, parent_path)?,
            UnderflowAction::Both => {
                if self
                    .try_borrow_right_v2(child_page_no, parent_path)
                    .is_err()
                {
                    self.try_borrow_left_v2(child_page_no, parent_path)?;
                }
            }
        };
        Ok(())
    }

    // fn try_borrow_right(
    //     &mut self,
    //     child_page_no: PageNo,
    //     parent_path: ActivePath,
    // ) -> SqliteResult<Option<()>> {
    //     let mut parent_page_guard = self.pager.get_mut(parent_path.page_no)?;
    //     let mut parent_page = self.page_as_mut(parent_path.page_no, &mut parent_page_guard)?;
    //     debug_assert!(
    //         parent_path.cell_idx < parent_page.no_of_cells(),
    //         "Right most pointer has no right sibling"
    //     );
    //     let sibling_idx = parent_path.cell_idx + 1;
    //     let sib_page_no = {
    //         if sibling_idx < parent_page.no_of_cells() {
    //             parent_page.cell(sibling_idx)?.left_child()
    //         } else {
    //             parent_page.right_most_ptr().unwrap()
    //         }
    //     };
    //     let mut sibling_page_guard = self.pager.get_mut(sib_page_no)?;
    //     let mut sibling_page = self.page_as_mut(sib_page_no, &mut sibling_page_guard)?;
    //     // dbg!(&sibling_page);
    //     // dbg!(sibling_page.freespace());
    //     debug_assert!(
    //         !sibling_page.is_underflow()?,
    //         "Right sibling page (PageNumber: {}) is underflow before borrowing",
    //         sib_page_no
    //     );
    //     let cell_span = sibling_page.cell_span(sibling_page.as_ref()?.get_cell_offset(0)?)?;
    //     if sibling_page
    //         .as_ref()?
    //         .would_underflow_after_remove(cell_span.end - cell_span.start)?
    //     {
    //         return Ok(None);
    //     }
    //     let sibling_cell = sibling_page.cell(0)?;
    //     let sibling_cell_bytes = sibling_page.cell_bytes_as_ref(0)?.to_owned();
    //     sibling_page.remove_cell(0);
    //     // move to the current cell
    //     self.with_page_mut(child_page_no, |page| {
    //         page.insert_cell(&sibling_cell_bytes, page.no_of_cells())?;
    //         if page.is_underflow()? {
    //             panic!("WE HAVE OVERFLOW EVEN AFTER BORROW FROM RIGHT");
    //         }
    //         Ok(())
    //     })?;

    //     // MOVE TO PARENT
    //     // TODO: CHECK IF WE CAN REPLACE IN PLACE
    //     let new_bytes = Encode::encode_table_interior_cell(child_page_no, sibling_cell.row_id());
    //     parent_page.remove_cell(parent_path.cell_idx)?;
    //     parent_page.insert_cell(&new_bytes, parent_path.cell_idx)?;

    //     Ok(Some(()))
    // }

    // fn try_borrow_left(
    //     &mut self,
    //     child_page_no: PageNo,
    //     parent_path: ActivePath,
    // ) -> SqliteResult<Option<()>> {
    //     let mut parent_page_guard = self.pager.get_mut(parent_path.page_no)?;
    //     let mut parent_page = self.page_as_mut(parent_path.page_no, &mut parent_page_guard)?;
    //     debug_assert!(
    //         parent_path.cell_idx > 0 && parent_path.cell_idx <= parent_page.no_of_cells(),
    //         "Left most pointer has no left sibling"
    //     );

    //     let sibling_idx = parent_path.cell_idx - 1;
    //     let sibling_cell = parent_page.cell(sibling_idx)?;

    //     let sib_page_no = sibling_cell.left_child();
    //     let mut sibling_page_guard = self.pager.get_mut(sib_page_no)?;
    //     let mut sibling_page = self.page_as_mut(sib_page_no, &mut sibling_page_guard)?;
    //     dbg!(&sibling_page);
    //     dbg!(sibling_page.freespace());
    //     debug_assert!(
    //         !sibling_page.is_underflow()?,
    //         "Left sibling page (PageNumber: {}) is underflow before borrowing",
    //         sib_page_no
    //     );
    //     let cell_to_borrow_index = sibling_page.no_of_cells() - 1;
    //     let cell_size = sibling_page.cell_size(cell_to_borrow_index)?;
    //     if sibling_page
    //         .as_ref()?
    //         .would_underflow_after_remove(cell_size)?
    //     {
    //         return Ok(None);
    //     }
    //     let sibling_cell = sibling_page.cell(cell_to_borrow_index)?;
    //     let sibling_cell_bytes = sibling_page
    //         .cell_bytes_as_ref(cell_to_borrow_index)?
    //         .to_owned();
    //     sibling_page.remove_cell(cell_to_borrow_index);
    //     // move to the current cell
    //     self.with_page_mut(child_page_no, |page| {
    //         page.insert_cell(&sibling_cell_bytes, 0)?;

    //         if page.is_underflow()? {
    //             todo!("WE HAVE OVERFLOW EVEN AFTER BORROW");
    //         }

    //         Ok(())
    //     })?;

    //     // MOVE TO PARENT
    //     // TODO: CHECK IF WE CAN REPLACE IN PLACE
    //     let new_bytes =
    //         Encode::encode_table_interior_cell(sibling_page.page_no, sibling_cell.row_id());
    //     parent_page.remove_cell(parent_path.cell_idx - 1)?;
    //     parent_page.insert_cell(&new_bytes, parent_path.cell_idx - 1)?;

    //     Ok(Some(()))
    // }

    fn try_borrow_right_v2(
        &mut self,
        child_page_no: PageNo,
        parent_path: ActivePath,
    ) -> SqliteResult<()> {
        let mut parent_page_guard = self.pager.get_mut(parent_path.page_no)?;
        let mut parent_page = self.page_as_mut(parent_path.page_no, &mut parent_page_guard)?;
        debug_assert!(
            parent_path.cell_idx < parent_page.no_of_cells(),
            "Right most pointer has no right sibling"
        );
        let sibling_idx = parent_path.cell_idx + 1;
        let sib_page_no = {
            if sibling_idx < parent_page.no_of_cells() {
                parent_page.cell(sibling_idx)?.left_child()
            } else {
                parent_page.right_most_ptr().unwrap()
            }
        };
        let mut sibling_page_guard = self.pager.get_mut(sib_page_no)?;
        let mut sibling_page = self.page_as_mut(sib_page_no, &mut sibling_page_guard)?;
        // debug_assert!(
        //     !sibling_page.is_underflow()?,
        //     "Right sibling page (PageNumber: {}) is underflow before borrowing",
        //     sib_page_no
        // );
        let mut current_page_guard = self.pager.get_mut(child_page_no)?;
        let mut current_page = self.page_as_mut(child_page_no, &mut current_page_guard)?;
        let mut all_cells_as_bytes: Vec<Vec<u8>> = Vec::new();
        let mut total_size_in_bytes = 0;
        for i in 0..current_page.no_of_cells() {
            let bytes = current_page.cell_bytes_as_ref(i)?.to_vec();
            total_size_in_bytes += bytes.len();
            all_cells_as_bytes.push(bytes);
        }
        let current_page_len = current_page.no_of_cells() as usize;
        for i in 0..sibling_page.no_of_cells() {
            let bytes = sibling_page.cell_bytes_as_ref(i)?.to_vec();
            total_size_in_bytes += bytes.len();
            all_cells_as_bytes.push(bytes);
        }

        // Check if they can fit in one page (cells + pointers + header)
        let total_cells = all_cells_as_bytes.len();
        let header_sz = current_page.header_size() as usize;
        let required = total_size_in_bytes + total_cells * 2 + header_sz;
        if required <= self.pager.metadata.usable_size {
            self.merge(
                all_cells_as_bytes,
                &mut sibling_page,
                parent_path.cell_idx,
                &mut parent_page,
                child_page_no,
            )?;
            return Ok(());
        }

        let target = total_size_in_bytes / 2;
        let mut split_at = 0;
        let mut running_size = 0;
        for (i, cell) in all_cells_as_bytes.iter().enumerate() {
            running_size += cell.len();
            if running_size >= target {
                split_at = i + 1;
                break;
            }
        }
        // Byte-split alone can leave a side empty; keep both sides non-empty.
        // Interior path clamps further below (needs a cell to promote).
        if total_cells < 2 {
            return Err(SqliteError::Internal(
                "cannot redistribute: not enough cells".into(),
            ));
        }
        split_at = split_at.clamp(1, total_cells - 1);
        let (new_left_page_cell, new_right_page_cells) = all_cells_as_bytes.split_at_mut(split_at);
        // last cell of the left share
        // this is the one whose row_id becomes the separator
        let separator_index = split_at - 1;

        // let separator_cell = if separator_index < current_page_len {
        //     // it's still one of current_page's original cells
        //     current_page.cell(separator_index as _)?
        // } else {
        //     // it's one of sibling_page's original cells
        //     sibling_page.cell((separator_index - current_page_len) as _)?
        // };
        /*
         * If the page we are rebalancing is a leaf page
         */
        if current_page.is_leaf() {
            debug_assert!(
                sibling_page.is_leaf(),
                "The current page is a leaf ({}), while its sibling page is an interior node ({}).",
                current_page.page_no,
                sib_page_no
            );
            current_page.reset_for_rebuild();
            for (i, bytes) in new_left_page_cell.iter().enumerate() {
                if current_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: left share does not fit".into(),
                    ));
                }
            }
            sibling_page.reset_for_rebuild();
            for (i, bytes) in new_right_page_cells.iter().enumerate() {
                if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: right share does not fit".into(),
                    ));
                }
            }
            debug_assert!(
                !current_page.is_underflow()?,
                "Current page still underflows after redistribution \
             (page_no: {}, free_space: {})",
                current_page.page_no,
                current_page.freespace()?
            );

            debug_assert!(
                !sibling_page.is_underflow()?,
                "Sibling page still underflows after redistribution \
             (page_no: {}, free_space: {})",
                sibling_page.page_no,
                sibling_page.freespace()?
            );

            // dbg!(current_page.cell(separator_index as _)?);
            // dbg!(separator_cell.row_id());

            // let new_bytes =
            //     Encode::encode_table_interior_cell(child_page_no, separator_cell.row_id());

            // let beta_cell = TableLeafCell::parse(
            //     &new_left_page_cell[separator_index],
            //     0,
            //     self.pager.metadata.usable_size as _,
            // );
            // dbg!(beta_cell);
            let new_bytes = Encode::encode_table_interior_cell(
                child_page_no,
                current_page.cell(separator_index as _)?.row_id(),
            );

            parent_page.remove_cell(parent_path.cell_idx)?;
            if parent_page.insert_cell(&new_bytes, parent_path.cell_idx)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "redistribute leaf: parent separator does not fit".into(),
                ));
            }
        } else {
            // Interior rotation promotes new_right[0] to the parent, so the
            // right share must keep at least 2 cells (promoted + remainder)
            // and the left must not shrink.
            if new_right_page_cells.len() < 2 || split_at < current_page_len {
                return Err(SqliteError::Internal(
                    "cannot redistribute interior: split leaves no promotable cell".into(),
                ));
            }
            let separator_cell = TableInteriorCell::parse(
                &new_right_page_cells[0],
                0,
                self.pager.metadata.usable_size,
            )
            .map(BTreeCell::TableInterior)?;
            debug_assert_eq!(
                current_page.header.page_kind, sibling_page.header.page_kind,
                "Current page kind ({:?}) does not match sibling page kind ({:?})",
                current_page.header.page_kind, sibling_page.header.page_kind,
            );

            let parent_separator_cell = parent_page.cell(parent_path.cell_idx)?;
            let parent_separator_cell_row_id_boundery = parent_separator_cell.row_id();
            // Sibling keeps its rightmost subtree; save before reset wipes it.
            let sibling_rmp = sibling_page.header.right_most_ptr;
            let new_cell_for_curr_page = Encode::encode_table_interior_cell(
                current_page.right_most_ptr().unwrap(),
                parent_separator_cell_row_id_boundery,
            );
            debug_assert!(
                current_page_len < new_left_page_cell.len(),
                "Redistributing Cells has no offect on the underflowed page"
            );
            let mut temp_offset = 0;
            current_page.reset_for_rebuild();
            for (i, bytes) in new_left_page_cell.iter().enumerate() {
                if i == current_page_len {
                    if current_page.insert_cell(&new_cell_for_curr_page, i as _)?
                        == InsertionState::None
                    {
                        return Err(SqliteError::Internal(
                            "redistribute interior: parent separator does not fit".into(),
                        ));
                    }
                    temp_offset = 1;
                }
                if current_page.insert_cell(bytes, (i + temp_offset) as _)? == InsertionState::None
                {
                    return Err(SqliteError::Internal(
                        "redistribute interior: left share does not fit".into(),
                    ));
                }
            }

            debug_assert_eq!(
                temp_offset, 1,
                "Separator key was not moved down as expected"
            );
            current_page.header.right_most_ptr = Some(separator_cell.left_child());
            let new_parent_cell =
                Encode::encode_table_interior_cell(child_page_no, separator_cell.row_id());
            parent_page.remove_cell(parent_path.cell_idx)?;
            if parent_page.insert_cell(&new_parent_cell, parent_path.cell_idx)?
                == InsertionState::None
            {
                return Err(SqliteError::Internal(
                    "redistribute interior: parent separator does not fit".into(),
                ));
            }
            sibling_page.reset_for_rebuild();
            // skip the cell we promote
            for (i, bytes) in new_right_page_cells[1..].iter().enumerate() {
                if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute interior: right share does not fit".into(),
                    ));
                }
            }
            if sibling_page.header.right_most_ptr != sibling_rmp {
                sibling_page.header.right_most_ptr = sibling_rmp;
                sibling_page.update_bytes([RightMostPointer]);
            }
            return Ok(());
        }
        Ok(())
    }

    fn try_borrow_left_v2(
        &mut self,
        child_page_no: PageNo,
        parent_path: ActivePath,
    ) -> SqliteResult<()> {
        let mut parent_page_guard = self.pager.get_mut(parent_path.page_no)?;
        let mut parent_page = self.page_as_mut(parent_path.page_no, &mut parent_page_guard)?;
        debug_assert!(
            parent_path.cell_idx > 0 && parent_path.cell_idx <= parent_page.no_of_cells(),
            "Left most pointer has no left sibling"
        );
        let sibling_idx = parent_path.cell_idx - 1;
        let sibling_cell = parent_page.cell(sibling_idx)?;
        let sib_page_no = sibling_cell.left_child();

        let mut sibling_page_guard = self.pager.get_mut(sib_page_no)?;
        let mut sibling_page = self.page_as_mut(sib_page_no, &mut sibling_page_guard)?;
        // debug_assert!(
        //     !sibling_page.is_underflow()?,
        //     "Left sibling page (PageNumber: {}) is underflow before borrowing",
        //     sib_page_no
        // );

        let mut current_page_guard = self.pager.get_mut(child_page_no)?;
        let mut current_page = self.page_as_mut(child_page_no, &mut current_page_guard)?;

        let mut all_cells_as_bytes: Vec<Vec<u8>> = Vec::new();
        let mut total_size_in_bytes = 0;
        // sibling (left, smaller keys) goes FIRST
        for i in 0..sibling_page.no_of_cells() {
            let bytes = sibling_page.cell_bytes_as_ref(i)?.to_vec();
            total_size_in_bytes += bytes.len();
            all_cells_as_bytes.push(bytes);
        }

        for i in 0..current_page.no_of_cells() {
            let bytes = current_page.cell_bytes_as_ref(i)?.to_vec();
            total_size_in_bytes += bytes.len();
            all_cells_as_bytes.push(bytes);
        }

        let total_cells = all_cells_as_bytes.len();
        let header_sz = current_page.header_size() as usize;
        let required = total_size_in_bytes + total_cells * 2 + header_sz;
        if required <= self.pager.metadata.usable_size {
            self.merge(
                all_cells_as_bytes,
                &mut current_page,
                sibling_idx,
                &mut parent_page,
                sib_page_no,
            )?;
            return Ok(());
        }

        let target = total_size_in_bytes / 2;
        let mut split_at = 0;
        let mut running_size = 0;
        for (i, cell) in all_cells_as_bytes.iter().enumerate() {
            running_size += cell.len();
            if running_size >= target {
                split_at = i + 1;
                break;
            }
        }
        if total_cells < 2 {
            return Err(SqliteError::Internal(
                "cannot redistribute: not enough cells".into(),
            ));
        }
        split_at = split_at.clamp(1, total_cells - 1);
        // sibling gets the LEFT half, current_page gets the RIGHT half
        let (new_sibling_cells, new_current_cells) = all_cells_as_bytes.split_at(split_at);

        if current_page.is_leaf() {
            debug_assert!(
                sibling_page.is_leaf(),
                "The current page is a leaf ({}), while its sibling page is an interior node ({}).",
                current_page.page_no,
                sib_page_no
            );
            let separator_index = split_at - 1;
            let sibling_len = sibling_page.no_of_cells() as usize;
            let separator_key = if separator_index < sibling_len {
                // it's still one of sibling_page's original cells
                sibling_page.cell(separator_index as _)?.row_id()
            } else {
                // it's one of current_page's original cells
                current_page
                    .cell((separator_index - sibling_len) as _)?
                    .row_id()
            };

            sibling_page.reset_for_rebuild();
            for (i, bytes) in new_sibling_cells.iter().enumerate() {
                if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: left share does not fit".into(),
                    ));
                }
            }
            current_page.reset_for_rebuild();
            for (i, bytes) in new_current_cells.iter().enumerate() {
                if current_page.insert_cell(bytes, i as _)? == InsertionState::None {
                    return Err(SqliteError::Internal(
                        "redistribute leaf: right share does not fit".into(),
                    ));
                }
            }

            debug_assert!(
                !sibling_page.is_underflow()?,
                "Sibling page still underflows after redistribution (page_no: {}, free_space: {})",
                sibling_page.page_no,
                sibling_page.freespace()?
            );
            debug_assert!(
                !current_page.is_underflow()?,
                "Current page still underflows after redistribution (page_no: {}, free_space: {})",
                current_page.page_no,
                current_page.freespace()?
            );

            // separator key = last key of sibling's new share, points to sibling (left child)
            let new_bytes = Encode::encode_table_interior_cell(sib_page_no, separator_key);
            parent_page.remove_cell(parent_path.cell_idx - 1)?;
            if parent_page.insert_cell(&new_bytes, parent_path.cell_idx - 1)?
                == InsertionState::None
            {
                return Err(SqliteError::Internal(
                    "redistribute leaf: parent separator does not fit".into(),
                ));
            }

            return Ok(());
        }

        // Interior mirror of try_borrow_right_v2: parent separator moves down
        // to the FRONT of the right page, sibling's last cell moves up.
        // Right share must keep >=1 cell, left share needs >=2 (promoted + remainder).
        if split_at < 2 || split_at > total_cells - 1 {
            return Err(SqliteError::Internal(
                "cannot redistribute interior: split leaves no promotable cell".into(),
            ));
        }
        debug_assert_eq!(
            current_page.header.page_kind, sibling_page.header.page_kind,
            "Current page kind ({:?}) does not match sibling page kind ({:?})",
            current_page.header.page_kind, sibling_page.header.page_kind,
        );
        let current_len = current_page.no_of_cells() as usize;
        let right_final = (total_cells - split_at) + 1;
        if right_final <= current_len {
            return Err(SqliteError::Internal(
                "cannot redistribute interior: split does not grow underflowed page".into(),
            ));
        }
        // Promoted cell = last of left share. Parse before rebuilds overwrite.
        let promoted_cell = TableInteriorCell::parse(
            &new_sibling_cells[new_sibling_cells.len() - 1],
            0,
            self.pager.metadata.usable_size,
        )
        .map(BTreeCell::TableInterior)?;
        let parent_separator_cell = parent_page.cell(sibling_idx)?;
        let parent_boundary = parent_separator_cell.row_id();
        // Current keeps its rightmost subtree; save before reset wipes it.
        // (Sibling's new RMP is set to the promoted cell's left child below.)
        let current_rmp = current_page.header.right_most_ptr;
        // Parent separator moves down front of right page; its left child is
        // the sibling's old right-most pointer.
        let new_cell_for_right = Encode::encode_table_interior_cell(
            sibling_page.right_most_ptr().unwrap(),
            parent_boundary,
        );

        sibling_page.reset_for_rebuild();
        for (i, bytes) in new_sibling_cells[..new_sibling_cells.len() - 1]
            .iter()
            .enumerate()
        {
            if sibling_page.insert_cell(bytes, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "redistribute interior: left share does not fit".into(),
                ));
            }
        }
        sibling_page.header.right_most_ptr = Some(promoted_cell.left_child());
        let new_parent_cell =
            Encode::encode_table_interior_cell(sib_page_no, promoted_cell.row_id());
        parent_page.remove_cell(sibling_idx)?;
        if parent_page.insert_cell(&new_parent_cell, sibling_idx)? == InsertionState::None {
            return Err(SqliteError::Internal(
                "redistribute interior: parent separator does not fit".into(),
            ));
        }
        current_page.reset_for_rebuild();
        if current_page.insert_cell(&new_cell_for_right, 0 as _)? == InsertionState::None {
            return Err(SqliteError::Internal(
                "redistribute interior: parent separator does not fit".into(),
            ));
        }
        for (i, bytes) in new_current_cells.iter().enumerate() {
            if current_page.insert_cell(bytes, (i + 1) as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "redistribute interior: right share does not fit".into(),
                ));
            }
        }
        if current_page.header.right_most_ptr != current_rmp {
            current_page.header.right_most_ptr = current_rmp;
            current_page.update_bytes([RightMostPointer]);
        }

        Ok(())
    }

    fn merge(
        &mut self,
        all_cells_as_bytes: Vec<Vec<u8>>,
        right_page: &mut BTreePageMut,
        separator_index: CellIndex,
        parent_page: &mut BTreePageMut,
        abandoned: PageNo,
    ) -> SqliteResult<()> {
        // Interior merge drops the parent separator; the merged page keeps
        // the RIGHT page's right-most pointer (its subtree is the rightmost).
        // Without this the page keeps the transient 0 written by reset and
        // the next seek follows RMP 0 -> "page number cannot be zero".
        let merged_rmp = right_page.header.right_most_ptr;
        right_page.reset_for_rebuild();
        for (i, bytes) in all_cells_as_bytes.iter().enumerate() {
            if right_page.insert_cell(bytes, i as _)? == InsertionState::None {
                return Err(SqliteError::Internal(
                    "merge: combined cells do not fit in one page".into(),
                ));
            }
        }
        if right_page.header.right_most_ptr != merged_rmp {
            right_page.header.right_most_ptr = merged_rmp;
            right_page.update_bytes([RightMostPointer]);
        }

        parent_page.remove_cell(separator_index)?;
        if parent_page.is_underflow()? {
            let parent_no = parent_page.page_no;
            self.fix_page_underflow(parent_no)?;
        }
        // All content now lives in right_page; the left page is garbage.
        // Free it only on the success path so a failed rebalance never
        // frees a page the tree still references.
        self.deallocate_page(abandoned)?;

        Ok(())
    }
}

pub fn page_as_ref_with_pager<'b, P: crate::vfs::file::SqliteFile>(
    page_no: PageNo,
    guard: &'b PageGuard,
    pager: &Pager<P>,
) -> Result<BTreePageRef<'b>, SqliteError> {
    BTreePageRef::new(
        page_no,
        guard.bytes_as_ref(),
        pager.metadata.page_size,
        pager.metadata.usable_size,
    )
}

pub fn page_as_mut_with_pager<'b, P: crate::vfs::file::SqliteFile>(
    page_no: PageNo,
    guard: &'b mut PageGuard,
    pager: &Pager<P>,
) -> Result<BTreePageMut<'b>, SqliteError> {
    BTreePageMut::new(
        page_no,
        guard.bytes_as_mut().unwrap(),
        pager.metadata.page_size,
        pager.metadata.usable_size,
    )
}
#[derive(Debug, Clone, Copy)]
struct ActivePath {
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

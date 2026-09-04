use super::cell::BTreeCell;
use super::cell::Encode;
use super::page::BTreePageMut;
use super::page::BTreePageOps;
use super::page::BTreePageRef;
use super::page::InsertionState;
use super::page::PageField::*;
use super::page::RIGHT_MOST_POINTER_SIZE;
use std::cell::Cell;
use std::fmt::Debug;

use crate::SqliteResult;
use crate::pager::pager::PageNo;

use crate::SqliteCursor;
use crate::SqliteError;

use crate::pager::guard::PageGuard;
use crate::pager::pager::Pager;
use crate::record::SqlType;
use crate::record::Value;
use crate::storage::page::BTreePageType;
use crate::storage::page::compute_table_local_payload_size;
use crate::util::sqlite_assert_with_corrupt_err;
use crate::varint::encode_varint;
use crate::vfs::file::SqliteFile;

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
enum CellPosition {
    L,
    R,
    M,
}
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
                let mut is_leaf_empty = false;
                let cell_idx = if page.no_of_cells() == 0 {
                    is_leaf_empty = true;
                    0
                } else {
                    page.no_of_cells() - 1
                };
                self.add_path(page_no, cell_idx, guard);
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
            let child = page.right_most_ptr().ok_or(SqliteError::Corrupt(
                "interior page has no right-most child".into(),
            ))?;
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
            let left_last_ptr =
                new_left_page.cell_pointers.last().copied().ok_or_else(|| {
                    SqliteError::Corrupt("root split with an empty left leaf".into())
                })?;
            let rowid = new_left_page.parse_cell_at(left_last_ptr)?.row_id();
            let right_last_ptr = right_page.cell_pointers.last().copied().ok_or_else(|| {
                SqliteError::Corrupt("root split with an empty right leaf".into())
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
                return Err(SqliteError::Corrupt(
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
                return Err(SqliteError::Corrupt(
                    "left leaf page overflowed during split".into(),
                ));
            }
        }

        let left_last_ptr =
            left_page.cell_pointers.last().copied().ok_or_else(|| {
                SqliteError::Corrupt("left leaf page is empty after split".into())
            })?;
        let right_last_ptr =
            right_page.cell_pointers.last().copied().ok_or_else(|| {
                SqliteError::Corrupt("right leaf page is empty after split".into())
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
            .ok_or_else(|| SqliteError::Corrupt("interior split left half is empty".into()))?;
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
                return Err(SqliteError::Corrupt(
                    "new interior page overflowed during split".into(),
                ));
            }
        }
        new_page.header.right_most_ptr = Some(cell_to_be_promoted.left_child());

        new_page.update_bytes([RightMostPointer]);

        interior_page.reset_for_rebuild();
        for (i, cell) in right_cells.iter().enumerate() {
            if interior_page.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Corrupt(
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
        let max_allocated_pages = self.pager.metadata.max_allocated_pages;
        let new_page_no = max_allocated_pages + 1;
        let new_len = self.pager.metadata.page_size * (max_allocated_pages + 1);
        self.pager.source.set_len(new_len)?;
        self.pager.metadata.max_allocated_pages += 1;
        self.update_max_allocated_pages()?;
        Ok(new_page_no as _)
    }
    // TODO: CACHE DATABASE HEADER AS WE NEED TO READ AND WRITE CONSTENTLY FROM IT
    pub fn update_max_allocated_pages(&mut self) -> Result<(), SqliteError> {
        let mut guard = self.pager.get_mut(1)?;
        let bytes = guard.bytes_as_mut().unwrap();
        bytes[DATABASE_SIZE_IN_PAGES_OFFSET
            ..DATABASE_SIZE_IN_PAGES_OFFSET + DATABASE_SIZE_IN_PAGES_SIZE]
            .copy_from_slice(&(self.pager.metadata.max_allocated_pages as u32).to_be_bytes());

        Ok(())
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
        let res = self.cursor.seek(self.pager, key.clone())?;
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
        let is_underflow = self.with_page_mut::<_, bool>(page_no, |page| {
            let is_undeflow = page.is_underflow()?;
            Ok(is_undeflow)
        })?;
        if page_no == self.root_page {
            return Ok(());
        }
        if is_underflow {
            println!("PageNo: {} had an overflow", page_no);
            std::process::exit(1)
        }

        Ok(())
    }
    /*
     * THIS FUNCTION RELIES ON THE UNDERFLOW PAGE BEING THE LAST ENTRY
     * IN THE PATH. WE MUST ENSURE THE PATH IS POSITIONED
     * AT THE PAGE CURRENTLY BEING REPAIRED.
     */
    pub fn fix_page_underflow(&mut self, page_no: PageNo) -> SqliteResult<()> {
        /*
         * TO FIX UNDERFLOW ON A PAGE
         * WE REQUIRE AT LEAST THE PAGE IT SELF AND ITS PARENT
         */
        debug_assert!(
            self.cursor.stack.len() >= 2,
            "Fix underflow function called on empty path stack"
        );
        self.cursor.stack.pop();
        let (parent_page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();
        let max_cells = self.with_page_ref(page_no, |page| Ok(page.no_of_cells()))?;
        let undeflow_action = self.underflow_planner(cell_idx, max_cells);
        let path = ActivePath::from(self.cursor.stack.as_ref());
        self.try_fix_underflow(undeflow_action)?;

        Ok(())
    }
    fn underflow_planner(&self, cell_idx: CellIndex, max_cells: u16) -> UnderflowAction {
        match cell_idx {
            0 => UnderflowAction::BorrowRight,
            max_cells => UnderflowAction::BorrowLeft,
            _ => UnderflowAction::Both,
        }
    }
    fn try_fix_underflow(&mut self, underflow_action: UnderflowAction) -> SqliteResult<()> {
        Ok(())
    }

    fn try_borrow_right(
        &mut self,
        child_page_no: PageNo,
        parent_path: ActivePath,
    ) -> SqliteResult<Option<()>> {
        let mut parent_page_guard = self.pager.get_mut(parent_path.page_no)?;
        let mut parent_page = self.page_as_mut(parent_path.page_no, &mut parent_page_guard)?;
        debug_assert!(
            parent_path.cell_idx < parent_page.no_of_cells(),
            "Right most pointer has no right sibling"
        );

        let sibling_idx = parent_path.cell_idx + 1;
        let sibling_cell = parent_page.cell(sibling_idx)?;

        let sib_page_no = sibling_cell.left_child();
        let mut sibling_page_guard = self.pager.get_mut(sib_page_no)?;
        let mut sibling_page = self.page_as_mut(sib_page_no, &mut sibling_page_guard)?;
        debug_assert!(
            !sibling_page.is_underflow()?,
            "Right sibling page (PageNumber: {}) is underflow before borrowing",
            sib_page_no
        );
        let cell_span = sibling_page.cell_span(sibling_page.as_ref()?.get_cell_offset(0)?)?;
        if sibling_page
            .as_ref()?
            .is_underflow_after_sub(cell_span.end - cell_span.start)?
        {
            return Ok(None);
        }
        let sibling_cell = sibling_page.cell(0)?;
        let sibling_cell_bytes = sibling_page.cell_bytes_as_ref(0)?.to_owned();
        sibling_page.remove_cell(0);
        // move to the current cell
        self.with_page_mut(child_page_no, |page| {
            page.insert_cell(&sibling_cell_bytes, page.no_of_cells())?;
            Ok(())
        })?;

        // MOVE TO PARENT
        // TODO: CHECK IF WE CAN REPLACE IN PLACE
        let new_bytes = Encode::encode_table_interior_cell(child_page_no, sibling_cell.row_id());
        parent_page.remove_cell(parent_path.cell_idx)?;
        parent_page.insert_cell(&new_bytes, parent_path.cell_idx)?;

        Ok(Some(()))
    }

    fn try_borrow_left(
        &mut self,
        child_page_no: PageNo,
        parent_path: ActivePath,
    ) -> SqliteResult<Option<()>> {
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
        debug_assert!(
            !sibling_page.is_underflow()?,
            "Left sibling page (PageNumber: {}) is underflow before borrowing",
            sib_page_no
        );
        let cell_to_borrow_index = sibling_page.no_of_cells() - 1;
        let cell_size = sibling_page.cell_size(cell_to_borrow_index)?;
        if sibling_page.as_ref()?.is_underflow_after_sub(cell_size)? {
            return Ok(None);
        }
        let sibling_cell = sibling_page.cell(cell_to_borrow_index)?;
        let sibling_cell_bytes = sibling_page
            .cell_bytes_as_ref(cell_to_borrow_index)?
            .to_owned();
        sibling_page.remove_cell(cell_to_borrow_index);
        // move to the current cell
        self.with_page_mut(child_page_no, |page| {
            page.insert_cell(&sibling_cell_bytes, 0)?;
            Ok(())
        })?;

        // MOVE TO PARENT
        // TODO: CHECK IF WE CAN REPLACE IN PLACE
        let new_bytes = Encode::encode_table_interior_cell(child_page_no, sibling_cell.row_id());
        parent_page.remove_cell(parent_path.cell_idx - 1)?;
        parent_page.insert_cell(&new_bytes, parent_path.cell_idx - 1)?;

        Ok(Some(()))
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

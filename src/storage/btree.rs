use std::cell::Cell;

use super::cell::BTreeCell;
use super::cell::Encode;
use super::page::BTreePageMut;
use super::page::BTreePageOps;
use super::page::BTreePageRef;
use super::page::InsertionState;
use crate::PageNo;
use crate::SqliteError;
use crate::format::page;
use crate::pager::guard::PageGuard;
use crate::pager::pager::Pager;
use crate::record::SqlType;
use crate::record::Value;
use crate::storage::page::BTreePageType;
use crate::util::sqlite_assert_with_corrupt_err;

pub const DATABASE_SIZE_IN_PAGES_OFFSET: usize = 28;
pub const DATABASE_SIZE_IN_PAGES_SIZE: usize = 4;
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
pub enum SearchResult {
    Found { row_id: i64, cell_index: CellIdx },
    Descend { child: u32, cell_index: CellIdx },
}
impl SearchResult {
    pub fn cell_index(&self) -> CellIdx {
        match self {
            Self::Found { cell_index, .. } => *cell_index,
            Self::Descend { cell_index, .. } => *cell_index,
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
    pub fn seek(
        &mut self,
        pager: &mut Pager,
        target: Value<'_>,
    ) -> Result<SeekResult, SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = BTree::page_as_ref_with_pager(page_no, &guard, pager)?;
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

            let page = BTree::page_as_ref_with_pager(page_no, &guard, pager)?;
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
            let page = BTree::page_as_ref_with_pager(page_no, &guard, pager)?;
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
            let page = BTree::page_as_ref_with_pager(page_no, &guard, pager)?;
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
            let page = BTree::page_as_ref_with_pager(page_no, &guard, pager)?;
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
    pub fn last(&mut self, pager: &mut Pager) -> Result<bool, SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        loop {
            let guard = pager.get(page_no)?;
            let page = BTree::page_as_ref_with_pager(page_no, &guard, pager)?;
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
                return Ok(is_leaf_empty);
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
            let page = BTree::page_as_ref_with_pager(page_no, &guard, pager)?;
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

            let page = BTree::page_as_ref_with_pager(*page_no, guard, pager)?;
            let cell = page.cell(*cell_idx)?;
            return Ok(Some(cell));
        }
        Ok(None)
    }
    fn clear_path(&mut self) {
        self.stack.clear();
    }
    fn choose_child<'g, P>(
        &self,
        page: &'g P,
        pager: &mut Pager,
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

    pub fn last_visited_entry(&self) -> Option<(u32, u16)> {
        if let Some(path) = self.stack.last() {
            return Some((path.page_no, path.cell_idx));
        }
        None
    }
    pub fn last_visited_entry_unchecked(&self) -> (u32, u16) {
        let p = self.stack.last().unwrap();
        (p.page_no, p.cell_idx)
    }

    fn choose_target<'a, P>(
        &self,
        page: &'a P,
        pager: &mut Pager,
        target: &Value<'_>,
    ) -> Result<(bool, CellIdx), SqliteError>
    where
        P: BTreePageOps<'a>,
    {
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
            let page = BTree::page_as_ref_with_pager(path.page_no, &path.guard, pager)?;
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

    fn add_path(&mut self, page_no: PageNo, cell_idx: CellIdx, guard: PageGuard) {
        self.stack.push(Path::new(page_no, cell_idx, guard));
    }
}

pub struct BTree<'a> {
    pub root_page: PageNo,
    pager: &'a mut Pager,
    pub cursor: BTreeCursor,
}

#[derive(Debug)]
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
impl<'a> BTree<'a> {
    pub fn new(root_page: PageNo, pager: &'a mut Pager) -> Self {
        Self {
            root_page,
            pager,
            cursor: BTreeCursor::new(root_page),
        }
    }
    pub fn with_cursor(root_page: PageNo, pager: &'a mut Pager, cursor: BTreeCursor) -> Self {
        Self {
            root_page,
            pager,
            cursor,
        }
    }

    pub fn insert(&mut self, key: Value, content: Vec<u8>) -> Result<(), SqliteError> {
        self.cursor.seek(self.pager, key.into_owned())?;
        let (page_no, cell_idx) = self.cursor.last_visited_entry_unchecked();
        let mut page_guard = self.pager.get_mut(page_no)?;
        let mut page = self.page_as_mut(page_no, &mut page_guard)?;
        if let InsertionState::Inserted = page.insert_cell(content.clone(), cell_idx)? {
            return Ok(());
        } else {
            let meta = self.balance(page_no)?;
            self.insert_key_to_leaf(&key, content, meta)?;
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
                .choose_child(&parent_page_as_ref, self.pager, &split_metadata.boundary)?
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
                        parent_page_as_mut.update_rmp();
                        Ok(split_metadata)
                    }
                    InsertionState::None => {
                        let meta = self.split_interior(parent_page_as_mut.page_no)?;
                        let key = split_metadata.boundary.into_owned();
                        self.insert_key_to_interior(&key, left_page_payload, meta)?;
                        Ok(split_metadata)
                    }
                }
            } else {
                // the old cell already points at the left page; only its key
                // becomes the boundary, then a divider for the right page follows
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
            let mut new_left_page = BTreePageMut::new_from_scratch(
                new_left_page_no,
                BTreePageType::LeafTable,
                new_left_page_guard.bytes_as_mut().unwrap(),
                self.pager.metadata.page_size,
                self.pager.metadata.usable_size,
            );
            new_left_page.copy_data_from(&left_page);
            // TODO REMOVE THIS LATER:
            left_page.clear();
            //
            // rebuild metadata
            let last_cell_ptr = left_page.cell_pointers.len() - 1;
            let rowid = new_left_page
                .get_page_as_ref()?
                .cell(last_cell_ptr as _)?
                .row_id();
            let right_max = right_page
                .get_page_as_ref()?
                .cell((right_page.cell_pointers.len() - 1) as _)?
                .row_id();

            let mut root = BTreePageMut::new_from_scratch(
                left_page.page_no,
                BTreePageType::InteriorTable,
                left_page_guard.bytes_as_mut().unwrap(),
                self.pager.metadata.page_size,
                self.pager.metadata.usable_size,
            );

            root.header.right_most_ptr = Some(right_page.page_no);
            root.update_rmp();

            let left_child_payload =
                Encode::encode_table_interior_cell(new_left_page_no, rowid as _);
            root.insert_cell(left_child_payload, 0)?;
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

        // TODO add freelist check
        let right_page_no = self.allocate_page()?;
        let mut right_page_guard = self.pager.get_mut(right_page_no)?;
        let mut right_page = BTreePageMut::new_from_scratch(
            right_page_no,
            BTreePageType::LeafTable,
            right_page_guard.bytes_as_mut().unwrap(),
            self.pager.metadata.page_size,
            self.pager.metadata.usable_size,
        );
        let right_cell_poiners = left_page
            .cell_pointers
            .split_off((left_page.cell_pointers.len() / 2) + 1);

        let mut prev = *left_page.cell_pointers.last().unwrap() as usize;
        left_page.header.no_of_cells = left_page.cell_pointers.len() as _;
        left_page.header.cell_content_area = prev as _;
        left_page.update_cell_pointers();
        left_page.update_cell_content_area();
        left_page.update_no_of_cells();

        #[allow(clippy::needless_range_loop)]
        for i in 0..right_cell_poiners.len() {
            let current = right_cell_poiners[i] as usize;
            let bytes = left_page.bytes[current..prev].as_ref();
            right_page.insert_cell(bytes, i as _)?;
            prev = current;
        }

        let left_page_ref = left_page
            .get_page_as_ref()?
            .cell((left_page.cell_pointers.len() - 1) as _)?;
        let right_page_ref = right_page
            .get_page_as_ref()?
            .cell((right_page.cell_pointers.len() - 1) as _)?;
        let metadata = SplitMetadata::new(
            left_page.page_no,
            right_page.page_no,
            left_page_ref.row_id().into_sqlite_value() as _,
            right_page_ref.row_id().into_sqlite_value() as _,
        );

        Ok(metadata)
    }

    pub fn split_interior(&mut self, page_no: PageNo) -> Result<SplitMetadata, SqliteError> {
        // ORIGINAL PAGE
        let mut interior_page_guard = self.pager.get_mut(page_no)?;
        let mut interior_page = self.page_as_mut(page_no, &mut interior_page_guard)?;

        // TO BE LEFT
        let new_page_no = self.allocate_page()?;
        let mut new_page_guard = self.pager.get_mut(new_page_no)?;
        let mut new_page = self.page_as_mut(new_page_no, &mut new_page_guard)?;

        let mut cell_pointers = std::mem::take(&mut interior_page.cell_pointers);

        let current_page_cell_pointers = cell_pointers.split_off(cell_pointers.len() / 2);

        let last_cell_offset = cell_pointers.pop().unwrap();

        let mut prev = self.pager.metadata.usable_size;

        #[allow(clippy::needless_range_loop)]
        for i in 0..cell_pointers.len() {
            let current = cell_pointers[i] as usize;
            let bytes = &interior_page.bytes[current..prev];
            new_page.insert_cell(bytes, i as _)?;
            prev = current;
        }

        let cell_to_be_promoted = interior_page
            .get_page_as_ref()?
            .cell(cell_pointers.len() as _)?;

        new_page.header.right_most_ptr = Some(cell_to_be_promoted.left_child());
        new_page.update_rmp();

        prev = last_cell_offset as _;
        #[allow(clippy::needless_range_loop)]
        for i in 0..current_page_cell_pointers.len() {
            let current = current_page_cell_pointers[i] as usize;
            //  OMPTIMAZE THIS
            // unsafe {
            //     let ptr = interior_page.bytes.as_ptr().add(current);
            //     let len = prev - current;
            //     interior_page.insert_cell_raw(ptr, len, i as _);
            // }
            //
            let bytes = interior_page.bytes[current..prev].to_vec();
            interior_page.insert_cell(&bytes, i as _)?;
            prev = current;
        }

        // PROMOTE KEY STAGE
        //

        let promoted_cell_payload =
            Encode::encode_table_interior_cell(new_page.page_no, cell_to_be_promoted.row_id() as _);
        let promoted_key = cell_to_be_promoted.row_id().into_sqlite_value();

        if let Some(path) = self.cursor.stack.pop() {
            let mut parent_guard = self.pager.get_mut(path.page_no)?;
            let mut parent_page = self.page_as_mut(path.page_no, &mut parent_guard)?;

            let page_as_ref = parent_page.get_page_as_ref()?;
            let cell_idx = self
                .cursor
                .choose_child(&page_as_ref, self.pager, &promoted_key)?
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
            // AGAIN WE ARE THE ROOOOOT
            //
            let new_right_page_no = self.allocate_page()?;
            let mut new_right_page_guard = self.pager.get_mut(new_right_page_no)?;
            let mut new_right_page =
                self.page_as_mut(new_right_page_no, &mut new_right_page_guard)?;

            new_right_page.copy_data_from(&interior_page);
            interior_page.clear();
            let mut root = BTreePageMut::new(
                interior_page.page_no,
                interior_page_guard.bytes_as_mut().unwrap(),
                self.pager.metadata.page_size,
                self.pager.metadata.usable_size,
            )?;

            root.header.right_most_ptr = Some(new_right_page_no);
            root.update_rmp();

            root.insert_cell(promoted_cell_payload, 0)?;

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
            .choose_child(&page_mut, self.pager, key)?
            .cell_index();
        page_mut.insert_cell(payload, cell_idx)?;
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
        let (_, cell_idx) = self.cursor.choose_target(&page_mut, self.pager, key)?;
        page_mut.insert_cell(payload, cell_idx)?;
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
        // dbg!(&guard, page_no);
        BTreePageMut::new(
            page_no,
            guard.bytes_as_mut().unwrap(),
            self.pager.metadata.page_size,
            self.pager.metadata.usable_size,
        )
    }

    fn page_as_ref_with_pager<'b>(
        page_no: PageNo,
        guard: &'b PageGuard,
        pager: &Pager,
    ) -> Result<BTreePageRef<'b>, SqliteError> {
        BTreePageRef::new(
            page_no,
            guard.bytes_as_ref(),
            pager.metadata.page_size,
            pager.metadata.usable_size,
        )
    }

    fn page_as_mut_with_pager<'b>(
        page_no: PageNo,
        guard: &'b mut PageGuard,
        pager: &Pager,
    ) -> Result<BTreePageMut<'b>, SqliteError> {
        BTreePageMut::new(
            page_no,
            guard.bytes_as_mut().unwrap(),
            pager.metadata.page_size,
            pager.metadata.usable_size,
        )
    }

    // TODO:
    //     USE FREE LIST AS FIRST THING TO CHECK BEFORE RUSHING
    //     INTO THE DISK
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
}

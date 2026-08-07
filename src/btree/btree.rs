use super::cell::BTreeCell;
use super::cell::IndexInteriorCell;
use super::cell::IndexLeafCell;
use super::cell::TableInteriorCell;
use super::cell::TableLeafCell;
use super::sqlite_cursor::SqliteCursor;
use crate::pager::guard::PageGuard;
use crate::pager::page::Pager;
use crate::util::sqlite_assert_with_corrupt_err;
use crate::vfs::file::SqliteFile;
use crate::PageNo;
use crate::SqliteError;
use std::marker::PhantomData;

pub const LEAF_BTREE_PAGE_HEADER_SIZE: u8 = 8;
pub const INTERIOR_BTREE_PAGE_HEADER_SIZE: u8 = 12;

pub const BTREE_TYPE_PAGE_OFFSET: u8 = 0;
pub const BTREE_TYPE_PAGE_SIZE: u8 = 1;

pub const FIRST_FREEBLOCK_OFFSET: usize = 1;
pub const FIRST_FREEBLOCK_SIZE: usize = 2;

pub const CELL_COUNT_OFFSET: usize = 3;
pub const CELL_COUNT_SIZE: usize = 2;

pub const CELL_CONTENT_AREA_OFFSET: usize = 5;
pub const CELL_CONTENT_AREA_SIZE: usize = 2;

pub const FRAGMENTED_FREE_BYTES_OFFSET: usize = 7;
pub const FRAGMENTED_FREE_BYTES_SIZE: usize = 1;

pub const RIGHT_MOST_POINTER_OFFSET: usize = 8;
pub const RIGHT_MOST_POINTER_SIZE: usize = 4;

pub type CellIdx = u16;
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BTreePageType {
    InteriorIndex = 0x02,
    LeafIndex = 0x0a,
    InteriorTable = 0x05,
    LeafTable = 0x0d,
}

impl BTreePageType {
    fn get(kind: u8) -> Option<Self> {
        match kind {
            0x0a => Some(Self::LeafIndex),
            0x02 => Some(Self::InteriorIndex),
            0x0d => Some(Self::LeafTable),
            0x05 => Some(Self::InteriorTable),
            _ => None,
        }
    }
    fn is_leaf(&self) -> bool {
        matches!(self, Self::LeafIndex | Self::LeafTable)
    }

    fn is_interior(&self) -> bool {
        matches!(self, Self::InteriorTable | Self::InteriorIndex)
    }
}

#[derive(Debug)]
pub struct BTreePageHeader {
    page_kind: BTreePageType,
    first_freeblock: u16,
    no_of_cells: u16,
    cell_content_area: u16,
    frag_cnt: u8,
    right_most_ptr: Option<PageNo>,
}
impl BTreePageHeader {
    fn parse(bytes: &[u8]) -> Result<Self, SqliteError> {
        let mut cursor = SqliteCursor::new(bytes);
        let page_kind_byte = cursor.read_next_u8()?;
        let p_kind = match BTreePageType::get(page_kind_byte) {
            Some(x) => x,
            _ => return Err(SqliteError::InvalidPageType(page_kind_byte)),
        };

        Self::parse_page(&mut cursor, p_kind)
    }
    fn parse_page(
        cursor: &mut SqliteCursor,
        page_kind: BTreePageType,
    ) -> Result<Self, SqliteError> {
        let first_freeblock: u16 = cursor.read_next_u16()?;
        let no_of_cells: u16 = cursor.read_next_u16()?;
        let cell_content_area: u16 = cursor.read_next_u16()?;
        let frag_cnt: u8 = cursor.read_next_u8()?;
        let is_interior = page_kind.is_interior();
        let right_most_ptr: Option<PageNo> = if is_interior {
            Some(cursor.read_next_u32()?)
        } else {
            None
        };
        let page_header = Self {
            page_kind,
            first_freeblock,
            no_of_cells,
            cell_content_area,
            frag_cnt,
            right_most_ptr,
        };
        Ok(page_header)
    }
    fn right_most_ptr(&self) -> Option<PageNo> {
        self.right_most_ptr
    }
    fn header_size(&self) -> u8 {
        if self.page_kind.is_interior() {
            return INTERIOR_BTREE_PAGE_HEADER_SIZE;
        }
        LEAF_BTREE_PAGE_HEADER_SIZE
    }
}

pub struct BTreePageRef<'p> {
    header: BTreePageHeader,
    bytes: &'p [u8],
    size: usize,
    usable_size: usize,
    _marker: PhantomData<&'p PageGuard>,
}
impl<'p> BTreePageRef<'p> {
    pub fn new(
        bytes: &'p [u8],
        _guard: &'p PageGuard,
        page_size: usize,
        usable_size: usize,
    ) -> Result<Self, SqliteError> {
        let header = BTreePageHeader::parse(bytes)?;
        Ok(Self {
            header,
            bytes,
            size: page_size,
            usable_size,
            _marker: PhantomData,
        })
    }
    pub fn cell(&self, cell_idx: CellIdx) -> Result<BTreeCell, SqliteError> {
        let start = self.header_size() as u16;
        let end = start + self.no_of_cells() * 2;

        let cell_offset = (cell_idx * 2) + self.header_size() as u16;
        sqlite_assert_with_corrupt_err(
            cell_offset >= start && cell_offset < end && (cell_offset - start).is_multiple_of(2),
            "Cell Index Out of Bounds",
        )?;

        let mut cursor = SqliteCursor::with_offset(self.bytes, cell_offset as _)?;
        let cell_ptr = cursor.read_next_u16()?;

        let bytes = self.bytes;

        match self.header.page_kind {
            BTreePageType::InteriorTable => {
                TableInteriorCell::parse(bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::TableInterior)
            }
            BTreePageType::LeafTable => {
                TableLeafCell::parse(bytes, cell_ptr, self.usable_size).map(BTreeCell::TableLeaf)
            }

            BTreePageType::InteriorIndex => {
                IndexInteriorCell::parse(bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::IndexInterior)
            }
            BTreePageType::LeafIndex => {
                IndexLeafCell::parse(bytes, cell_ptr, self.usable_size).map(BTreeCell::IndexLeaf)
            }
        }
    }
    fn page_type(&self) -> BTreePageType {
        self.header.page_kind
    }
    fn header_size(&self) -> u8 {
        self.header.header_size()
    }
    fn is_leaf(&self) -> bool {
        self.header.page_kind.is_leaf()
    }

    fn is_interior(&self) -> bool {
        self.header.page_kind.is_interior()
    }
    fn right_most_ptr(&self) -> Option<PageNo> {
        self.header.right_most_ptr()
    }
    fn no_of_cells(&self) -> u16 {
        self.header.no_of_cells
    }
}

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

enum Step {
    Descend(PageNo),
    Leaf((bool, CellIdx)),
}

enum NextStep {
    Advance,
    Descend { entry_idx: CellIdx, child: PageNo },
    Pop,
    Previous,
}

#[derive(Debug)]
pub struct BTreeCursor {
    root: PageNo,
    pub stack: Vec<(PageNo, CellIdx)>,
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
    pub fn seek<P: SqliteFile>(
        &mut self,
        pager: &mut Pager<P>,
        target: u64,
    ) -> Result<SeekResult, SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        let (found, cell_idx) = loop {
            match Self::with_page(pager, page_no, |page| {
                if page.is_leaf() {
                    self.choose_target(page, target).map(Step::Leaf)
                } else {
                    let (child, idx) = self.choose_child(page, target)?;
                    self.stack.push((page_no, idx));
                    Ok(Step::Descend(child))
                }
            })? {
                Step::Leaf(r) => break r,
                Step::Descend(child) => page_no = child,
            }
        };
        self.stack.push((page_no, cell_idx));
        self.state = CursorState::At;
        if found {
            return Ok(SeekResult::Exact);
        }
        Ok(SeekResult::NotFound)
    }
    pub fn next<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        while let Some((page_no, cell_idx)) = self.stack.pop() {
            let step = Self::with_page(pager, page_no, |page| {
                if page.is_leaf() {
                    if cell_idx + 1 < page.no_of_cells() {
                        Ok(NextStep::Advance)
                    } else {
                        Ok(NextStep::Pop)
                    }
                } else if cell_idx + 1 == page.no_of_cells() {
                    let child = page.right_most_ptr().ok_or(SqliteError::Corrupt(
                        "interior page has no right-most child".into(),
                    ))?;
                    Ok(NextStep::Descend {
                        entry_idx: page.no_of_cells(),
                        child,
                    })
                } else if cell_idx + 1 < page.no_of_cells() {
                    let child = page.cell(cell_idx + 1)?.left_child();
                    Ok(NextStep::Descend {
                        entry_idx: cell_idx + 1,
                        child,
                    })
                } else {
                    Ok(NextStep::Pop)
                }
            })?;
            match step {
                NextStep::Advance => {
                    self.stack.push((page_no, cell_idx + 1));
                    self.state = CursorState::At;
                    return Ok(());
                }
                NextStep::Descend { entry_idx, child } => {
                    self.stack.push((page_no, entry_idx));
                    self.descend_to_first(pager, child)?;
                    return Ok(());
                }
                _ => {}
            }
        }
        self.state = CursorState::AfterLast;
        Ok(())
    }
    pub fn first<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        while let Some(child) = Self::with_page(pager, page_no, |page| {
            if page.is_leaf() {
                self.stack.push((page_no, 0));
                Ok(Option::None)
            } else {
                let child = page.cell(0)?.left_child();
                self.stack.push((page_no, 0));
                Ok(Option::Some(child))
            }
        })? {
            page_no = child
        }

        self.state = CursorState::At;
        Ok(())
    }
    pub fn descend_to_first<P: SqliteFile>(
        &mut self,
        pager: &mut Pager<P>,
        page_no: PageNo,
    ) -> Result<(), SqliteError> {
        let mut page_no = page_no;
        while let Some(child) = Self::with_page(pager, page_no, |page| {
            if page.is_leaf() {
                self.stack.push((page_no, 0));
                Ok(Option::None)
            } else {
                let child = page.cell(0)?.left_child();
                self.stack.push((page_no, 0));
                Ok(Option::Some(child))
            }
        })? {
            page_no = child
        }

        self.state = CursorState::At;
        Ok(())
    }
    pub fn prev<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        while let Some((page_no, cell_idx)) = self.stack.pop() {
            match Self::with_page(pager, page_no, |page| {
                if page.is_leaf() {
                    if cell_idx > 0 {
                        Ok(NextStep::Previous)
                    } else {
                        Ok(NextStep::Pop)
                    }
                } else {
                    if cell_idx > 0 {
                        let child = page.cell(cell_idx - 1)?.left_child();
                        Ok(NextStep::Descend {
                            entry_idx: cell_idx - 1,
                            child,
                        })
                    } else {
                        Ok(NextStep::Pop)
                    }
                }
            })? {
                NextStep::Previous => {
                    self.stack.push((page_no, cell_idx - 1));
                    return Ok(());
                }
                NextStep::Descend { entry_idx, child } => {
                    self.stack.push((page_no, entry_idx));
                    self.descend_to_last(pager, child)?;
                    return Ok(());
                }
                _ => {}
            }
        }
        self.state = CursorState::BeforeFirst;
        Ok(())
    }
    pub fn last<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        self.clear_path();
        let mut page_no = self.root;
        while let Some(child) = Self::with_page(pager, page_no, |page| {
            if page.is_leaf() {
                // last cell
                self.stack.push((page_no, page.no_of_cells() - 1));
                Ok(None) // to break
            } else {
                let child = page.right_most_ptr().ok_or(SqliteError::Corrupt(
                    "interior page has no right-most child".into(),
                ))?;
                self.stack.push((page_no, page.no_of_cells()));
                Ok(Some(child))
            }
        })? {
            page_no = child
        }

        self.state = CursorState::At;
        Ok(())
    }
    fn descend_to_last<P: SqliteFile>(
        &mut self,
        pager: &mut Pager<P>,
        page_no: PageNo,
    ) -> Result<(), SqliteError> {
        let mut page_no = page_no;
        while let Some(child) = Self::with_page(pager, page_no, |page| {
            if page.is_leaf() {
                self.stack.push((page_no, page.no_of_cells() - 1));
                Ok(None)
            } else {
                let child = page.right_most_ptr().ok_or(SqliteError::Corrupt(
                    "interior page has no right-most child".into(),
                ))?;
                self.stack.push((page_no, page.no_of_cells()));
                Ok(Some(child))
            }
        })? {
            page_no = child
        }

        self.state = CursorState::At;
        Ok(())
    }

    // NOTE: This works for now since [`BTreeCell`]
    // has no borrowed payload from its [`BTreePage`]
    //
    // TODO: Re-make this function after cell borrows
    // bytes from [`BTreePage`]
    pub fn current<P: SqliteFile>(
        &mut self,
        pager: &mut Pager<P>,
    ) -> Result<Option<BTreeCell>, SqliteError> {
        if let Some((page_no, cell_idx)) = self.stack.last() {
            #[rustfmt::skip]
            match Self::with_page(pager, *page_no, |page| {
                let cell = page.cell(0)?;
                Ok(Some(cell))
            })? {
                Some(cell) => return Ok(Some(cell)),
                _ => return Ok(None),
            };
        }
        Ok(None)
    }
    fn clear_path(&mut self) {
        self.stack.clear();
    }
    fn choose_child<'a>(
        &self,
        page: &BTreePageRef<'a>,
        target: u64,
    ) -> Result<(PageNo, CellIdx), SqliteError> {
        debug_assert!(
            page.is_interior(),
            "Navigation path of this works only with interior pages"
        );
        let bytes = page.bytes;
        let cell_count = page.no_of_cells();
        if page.page_type() == BTreePageType::InteriorTable {
            let mut l = 0;
            let mut r = cell_count;
            while l < r {
                let m = l + (r - l) / 2;
                let cell = page.cell(m)?;
                if cell.row_id() >= target {
                    return Ok((cell.left_child(), m));
                } else if cell.row_id() > target {
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
        target: u64,
    ) -> Result<(bool, CellIdx), SqliteError> {
        assert!(page.is_leaf(), "This navigation path works only for leaves");
        let cell_cnt = page.no_of_cells();
        if page.page_type() == BTreePageType::LeafTable {
            let mut l = 0;
            let mut r = cell_cnt;
            while l < r {
                let m: u16 = l + ((r - l) / 2);
                let cell = page.cell(m)?;
                if cell.row_id() == target {
                    return Ok((true, m));
                } else if cell.row_id() > target {
                    r = m;
                } else {
                    l = m + 1;
                }
            }
            return Ok((false, l));
        }
        todo!("INDEX LEAF NOT IMPLEMENTED YET")
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
            page_guard.bytes(),
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
        let (page_no, cell_idx) = *self.stack.last().unwrap();
        Self::with_page(pager, page_no, |page| {
            let cell = page.cell(cell_idx)?;
            f(page, &cell)
        })
    }

    fn page_as_ref<'a, P: SqliteFile>(
        guard: &'a PageGuard,
        pager: &Pager<P>,
    ) -> Result<BTreePageRef<'a>, SqliteError> {
        Ok(BTreePageRef::new(
            guard.bytes(),
            guard,
            pager.metadata.page_size,
            pager.metadata.usable_size,
        )?)
    }
}

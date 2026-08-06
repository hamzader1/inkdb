use super::cell::BTreeCell;
use super::cell::IndexInteriorCell;
use super::cell::IndexLeafCell;
use super::cell::TableInteriorCell;
use super::cell::TableLeafCell;
use super::sqlite_cursor::SqliteCursor;
use crate::bytes::*;
use crate::pager::guard::PageGuard;
use crate::pager::page::Pager;
use crate::to_int;
use crate::util::sqlite_assert_with_corrupt_err;
use crate::vfs::file::SqliteFile;
use crate::PageNo;
use crate::SqliteError;
use std::io::Cursor;
use std::io::Read;
use std::io::Seek;
use std::marker::PhantomData;
use std::ptr::NonNull;

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
        let page_kind_byte = cursor.read_next_u8();
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
        let first_freeblock: u16 = cursor.read_next_u16();
        let no_of_cells: u16 = cursor.read_next_u16();
        let cell_content_area: u16 = cursor.read_next_u16();
        let frag_cnt: u8 = cursor.read_next_u8();
        let is_interior = page_kind.is_interior();
        let right_most_ptr: Option<PageNo> = if is_interior {
            Some(cursor.read_next_u32())
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
    fn new(
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
    fn cell(&self, cell_idx: CellIdx) -> Result<BTreeCell, SqliteError> {
        let start = self.header_size() as u16;
        let end = start + self.no_of_cells() * 2;

        let cell_offset = (cell_idx * 2) + self.header_size() as u16;
        sqlite_assert_with_corrupt_err(
            cell_offset >= start && cell_offset < end && (cell_offset - start) % 2 == 0,
            "Cell Index Out of Bounds",
        )?;

        let mut cursor = SqliteCursor::with_offset(self.bytes, cell_offset as _);
        let cell_ptr = cursor.read_next_u16();

        let bytes = self.bytes;

        match self.header.page_kind {
            BTreePageType::InteriorTable => {
                return TableInteriorCell::parse(bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::TableInterior);
            }
            BTreePageType::LeafTable => {
                return TableLeafCell::parse(bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::TableLeaf);
            }

            BTreePageType::InteriorIndex => {
                return IndexInteriorCell::parse(bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::IndexInterior);
            }
            BTreePageType::LeafIndex => {
                return IndexLeafCell::parse(bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::IndexLeaf)
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

#[derive(Debug)]
enum CursorState {
    At,
    Invalid,
    AfterLast,
}

#[derive(Debug, PartialEq)]
pub enum SeekResult {
    Exact,
    NotFound,
}
#[derive(Debug)]
pub struct BTreeCursor {
    root: PageNo,
    stack: Vec<(PageNo, CellIdx)>,
    state: CursorState,
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
        let mut page_no = self.root;
        loop {
            let page_guard = pager.get(page_no)?;
            let page = BTreePageRef::new(
                page_guard.bytes(),
                &page_guard,
                pager.metadata.page_size,
                pager.metadata.usable_size,
            )?;
            if page.is_leaf() {
                // move to the next step
                break;
            }
            let (child_page, idx) = self.choose_child(page, target)?;
            self.stack.push((page_no, idx));
            page_no = child_page;
        }

        let page_guard = pager.get(page_no)?;
        let page = BTreePageRef::new(
            page_guard.bytes(),
            &page_guard,
            pager.metadata.page_size,
            pager.metadata.usable_size,
        )?;

        let (found, cell_idx) = self.choose_target(page, target)?;
        self.stack.push((page_no, cell_idx));
        self.state = CursorState::At;
        if found {
            return Ok(SeekResult::Exact);
        }
        Ok(SeekResult::NotFound)
    }
    pub fn next<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        while let Some(last_path) = self.stack.pop() {
            let (page_no, cell_idx) = last_path;
            let page_guard = pager.get(page_no)?;
            let page = BTreePageRef::new(
                page_guard.bytes(),
                &page_guard,
                pager.metadata.page_size,
                pager.metadata.usable_size,
            )?;

            if page.is_leaf() {
                if cell_idx + 1 < page.no_of_cells() {
                    self.stack.push((page_no, cell_idx + 1));
                    self.state = CursorState::At;
                    return Ok(());
                }
            }
            // Interior
            //
            if page.is_interior() {
                // if true, go to the RMC
                if cell_idx + 1 == page.no_of_cells() {
                    self.stack.push((page_no, page.no_of_cells()));
                    self.descend_to_first(page.right_most_ptr().unwrap(), pager)?;
                    return Ok(());
                } else if cell_idx + 1 < page.no_of_cells() {
                    let next_cell_idx = cell_idx + 1;
                    let left_child = page.cell(next_cell_idx)?.left_child();
                    self.stack.push((page_no, next_cell_idx));
                    self.descend_to_first(left_child, pager)?;
                    return Ok(());
                }
            }
        }
        self.state = CursorState::AfterLast;
        Ok(())
    }
    pub fn first<P: SqliteFile>(&mut self, pager: &mut Pager<P>) -> Result<(), SqliteError> {
        let mut page_no = self.root;
        self.clear_path();
        loop {
            let page_guard = pager.get(page_no)?;
            let page = BTreePageRef::new(
                page_guard.bytes(),
                &page_guard,
                pager.metadata.page_size,
                pager.metadata.usable_size,
            )?;
            if page.is_leaf() {
                break;
            }

            let child = page.cell(0)?.left_child();
            self.stack.push((page_no, 0));

            page_no = child;
        }

        let page_guard = pager.get(page_no)?;
        let page = BTreePageRef::new(
            page_guard.bytes(),
            &page_guard,
            pager.metadata.page_size,
            pager.metadata.usable_size,
        )?;
        // even if we won't use it right know
        // this must check if the cell is VALID
        page.cell(0)?;
        self.stack.push((page_no, 0));
        self.state = CursorState::At;
        Ok(())
    }
    
    fn clear_path(&mut self) {
        self.stack.clear();
    }
    fn choose_child<'a>(
        &self,
        page: BTreePageRef<'a>,
        target: u64,
    ) -> Result<(PageNo, CellIdx), SqliteError> {
        debug_assert!(
            page.is_interior(),
            "Navigation path of this works only with interior pages"
        );
        let bytes = page.bytes;
        let cell_count = page.no_of_cells();
        if page.page_type() == BTreePageType::InteriorTable {
            for i in 0..cell_count {
                let cell = page.cell(i)?;
                if cell.row_id() >= target {
                    return Ok((cell.left_child(), i));
                }
            }
            return Ok((page.right_most_ptr().unwrap(), cell_count));
        }
        todo!("INDEX INTERIOR NOT IMPLEMENTED YET")
    }

    fn choose_target<'a>(
        &self,
        page: BTreePageRef<'a>,
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
}

use super::page_cursor::PageCursor;
use crate::bytes::*;
use crate::pager::guard::PageGuard;
use crate::pager::page::Pager;
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
#[derive(Debug, Clone, Copy)]
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
        let mut r = Cursor::new(bytes);
        let page_kind_byte = read_u8(&mut r)?;
        let p_kind = match BTreePageType::get(page_kind_byte) {
            Some(x) => x,
            _ => return Err(SqliteError::InvalidPageType(page_kind_byte)),
        };

        Self::parse_page(&mut r, p_kind)
    }
    fn parse_page<R: Read + Seek>(
        r: &mut R,
        page_kind: BTreePageType,
    ) -> Result<Self, SqliteError> {
        let first_freeblock: u16 = read_u16_be(r)?;
        let no_of_cells: u16 = read_u16_be(r)?;
        let cell_content_area: u16 = read_u16_be(r)?;
        let frag_cnt: u8 = read_u8(r)?;
        let is_interior = page_kind.is_interior();
        let right_most_ptr: Option<PageNo> = if is_interior {
            Some(read_u32_be(r)?)
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

// should cache:
// page type
// cell pointer count for assertion?
// cell pointer array
struct BTreePageRef<'p> {
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
}

enum CursorState {
    At,
    Invalid,
    AfterLast,
}
enum SeekResult {
    Exact,
    NotFound,
}

struct BTreeCursor {
    root: PageNo,
    stack: Vec<(PageNo, CellIdx)>,
    state: CursorState,
}
impl BTreeCursor {
    fn new(root: PageNo) -> Self {
        Self {
            root,
            stack: Vec::new(),
            state: CursorState::Invalid,
        }
    }

    fn seek<P: SqliteFile>(
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
            let (child_page, idx) = self.choose_child(page, target);
            self.stack.push((page_no, idx));
            page_no = child_page;
        }

        Ok(SeekResult::Exact) // temp for now until we implement
    }
    fn choose_child<'a>(&self, page: BTreePageRef<'a>, target: u64) -> (PageNo, CellIdx) {
        debug_assert!(
            page.is_interior(),
            "Navigation path of this works only with interior pages"
        );
        let bytes = page.bytes;
        let cell_count = page.header.no_of_cells;
        let mut p_cursor = PageCursor::new_offset(bytes, page.header_size() as _);
        for i in 0..page.header.no_of_cells {
            let left_child_page = p_cursor.read_next_u32();
            let (rowid, _) = p_cursor.read_varint_next(page.usable_size);
            if rowid >= target {
                return (left_child_page, i);
            }
        }
        return (page.right_most_ptr().unwrap(), cell_count);
    }
}

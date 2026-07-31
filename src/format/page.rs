use super::header::SQLITE3_HEADER_SIZE;
use crate::bytes::{read_u16_be, read_u32_be};
use crate::format::cell::{IndexInteriorCell, IndexLeafCell, TableInteriorCell, TableLeafCell};
use crate::util::{sqlite_assert_one, sqlite_assert_with_corrupt_err};
use crate::{bytes::read_u8, errors::SqliteDatabaseError, format::cell::BTreeCell};
use crate::{decode_varint, seek_c, seek_s, sqlite_assert_all};
use std::io::{Cursor, Read, Seek, SeekFrom};

// pub const INTERIOR_INEDX_BTREE_PAGE: u8 = 0x02;
// pub const LEAF_INEDX_BTREE_PAGE: u8 = 0x0a;
//
// pub const INTERIOR_TABLE_BTREE_PAGE: u8 = 0x05;
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

pub type PageNumber = u32;

#[derive(Debug, Clone, Copy)]
pub struct CellPointer(u16);
impl CellPointer {
    pub fn get(&self) -> u16 {
        self.0
    }
}

#[derive(Debug)]
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

pub struct BTreePage {
    page_no: u32,
    header_offset: u16,
    header: BTreePageHeader,
    bytes: Vec<u8>,
    cell_pointers: Vec<CellPointer>,
    page_size: usize,
    usable_size: usize,
}
impl<'a> BTreePage {
    pub fn parse(
        mut bytes: Vec<u8>,
        page_no: u32,
        page_size: usize,
        usable_size: usize,
    ) -> Result<Self, SqliteDatabaseError> {
        let mut r = Cursor::new(&mut bytes);
        let mut header_offset: u16 = 0;
        if page_no == 1 {
            header_offset = SQLITE3_HEADER_SIZE as _;
            r.seek(SeekFrom::Start(SQLITE3_HEADER_SIZE.into()))?;
        }
        let header = BTreePageHeader::parse::<Cursor<&mut _>>(&mut r)?;

        let mut btree_page = BTreePage {
            page_no,
            header_offset,
            header,
            page_size,
            usable_size,
            bytes,
            cell_pointers: vec![],
        };
        btree_page.parse_cell_array_into_page()?;
        btree_page.validate(usable_size as u16)?; // fix usable_size later
        Ok(btree_page)
    }

    fn validate(&self, usable_size: u16) -> Result<(), SqliteDatabaseError> {
        let header_size = if self.header.page_kind.is_leaf() {
            LEAF_BTREE_PAGE_HEADER_SIZE
        } else {
            INTERIOR_BTREE_PAGE_HEADER_SIZE
        };
        let mut btree_header_offset = 0u8;
        if self.page_no == 0 {
            btree_header_offset = SQLITE3_HEADER_SIZE;
        }
        sqlite_assert_one(
            (btree_header_offset as u16 + header_size as u16) + (self.header.no_of_cells * 2)
                <= usable_size,
            SqliteDatabaseError::CorruptedPage {
                page: self.page_no,
                reason: "cell pointer array exceeds usable page space".into(),
            },
        )?;
        sqlite_assert_one(
            self.header.cell_content_area <= usable_size,
            SqliteDatabaseError::CorruptedPage {
                page: self.page_no,
                reason: "cell content area starts outside usable page space".into(),
            },
        )?;

        sqlite_assert_one(
            self.header.frag_cnt <= 60,
            SqliteDatabaseError::CorruptedPage {
                page: self.page_no,
                reason: "fragmented free-byte count exceeds 60".into(),
            },
        )?;
        let pointer_array_end = btree_header_offset as usize
            + header_size as usize
            + (self.header.no_of_cells as usize * 2);

        for CellPointer(cell_offset) in &self.cell_pointers {
            let cell_offset = *cell_offset as usize;

            sqlite_assert_one(
                cell_offset < usable_size as usize,
                SqliteDatabaseError::CorruptedPage {
                    page: self.page_no,
                    reason: format!("cell pointer {cell_offset} is outside usable page space"),
                },
            )?;

            sqlite_assert_one(
                cell_offset >= pointer_array_end,
                SqliteDatabaseError::CorruptedPage {
                    page: self.page_no,
                    reason: format!(
                        "cell pointer {cell_offset} points into the page header or cell pointer array"
                    ),
                },
            )?;
        }

        if self.header.page_kind.is_interior() {
            let right_most_child = (self.header.right_most_ptr).as_ref().ok_or(
                SqliteDatabaseError::CorruptedPage {
                    page: self.page_no,
                    reason: "interior page has no right-most child pointer".into(),
                },
            )?;

            sqlite_assert_one(
                *right_most_child != 0,
                SqliteDatabaseError::CorruptedPage {
                    page: self.page_no,
                    reason: "interior page has right-most child page 0".into(),
                },
            )?;
        }
        Ok(())
    }

    fn parse_cell_array_into_page(&mut self) -> Result<(), SqliteDatabaseError> {
        let Self {
            header_offset,
            header,
            cell_pointers,
            bytes,
            ..
        } = self;
        let mut cursor = Cursor::new(bytes);
        let header_size = header.get_header_size();
        cursor.seek(SeekFrom::Start(
            (*header_offset + header_size as u16) as u64,
        ))?;
        for _ in 0..header.no_of_cells {
            let cell_pointer = CellPointer(read_u16_be(&mut cursor)?);
            cell_pointers.push(cell_pointer);
        }
        Ok(())
    }

    pub fn cell(&self, cell_idx: u16) -> Result<BTreeCell, SqliteDatabaseError> {
        sqlite_assert_with_corrupt_err(
            (cell_idx as usize) < self.cell_pointers.len(),
            "Cell Index Out of Bounds",
        )?;
        let cell_ptr = self.cell_pointers[cell_idx as usize];
        // make sure the cell pointer inside the usable page
        sqlite_assert_with_corrupt_err(
            cell_ptr.get() as usize <= self.usable_size,
            "invalid cell pointer",
        )?;
        let mut r = Cursor::new(&self.bytes);

        match self.header.page_kind {
            BTreePageType::InteriorTable => {
                return TableInteriorCell::parse::<Cursor<_>>(&mut r, cell_ptr, self.usable_size)
                    .map(BTreeCell::TableInterior);
            }
            BTreePageType::LeafTable => {
                return TableLeafCell::parse::<Cursor<_>>(&mut r, cell_ptr, self.usable_size)
                    .map(BTreeCell::TableLeaf);
            }

            BTreePageType::InteriorIndex => {
                return IndexInteriorCell::parse(&mut r, cell_ptr, self.usable_size)
                    .map(BTreeCell::IndexInterior);
            }
            BTreePageType::LeafIndex => {
                return IndexLeafCell::parse(&mut r, cell_ptr, self.usable_size)
                    .map(BTreeCell::IndexLeaf)
            }
        }
    }

    pub fn bytes(&'a self) -> &'a Vec<u8> {
        &self.bytes
    }
    pub fn no_of_cell(&self) -> usize {
        self.header.no_of_cells as _
    }
}

#[derive(Debug)]
pub struct BTreePageHeader {
    page_kind: BTreePageType,
    first_freeblock: u16,
    no_of_cells: u16,
    cell_content_area: u16,
    frag_cnt: u8,
    right_most_ptr: Option<PageNumber>,
}
impl BTreePageHeader {
    fn parse<R: Read + Seek>(r: &mut R) -> Result<Self, SqliteDatabaseError> {
        let page_kind_byte = read_u8(r)?;
        let p_kind = match BTreePageType::get(page_kind_byte) {
            Some(x) => x,
            _ => return Err(SqliteDatabaseError::InvalidPageType(page_kind_byte)),
        };

        Self::parse_page(r, p_kind)
    }
    fn parse_page<R: Read + Seek>(
        r: &mut R,
        page_kind: BTreePageType,
    ) -> Result<Self, SqliteDatabaseError> {
        let first_freeblock: u16 = read_u16_be(r)?;
        let no_of_cells: u16 = read_u16_be(r)?;
        let cell_content_area: u16 = read_u16_be(r)?;
        let frag_cnt: u8 = read_u8(r)?;
        let is_interior = page_kind.is_interior();
        let right_most_ptr: Option<PageNumber> = if is_interior {
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
    fn get_header_size(&self) -> u8 {
        if self.page_kind.is_interior() {
            return INTERIOR_BTREE_PAGE_HEADER_SIZE;
        }
        LEAF_BTREE_PAGE_HEADER_SIZE
    }
}

use std::fmt;

impl fmt::Debug for BTreePage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BTreePage")
            .field("page_no", &self.page_no)
            .field("header_offset", &self.header_offset)
            .field("header", &self.header)
            .field("bytes_len", &self.bytes.len())
            .field("cell_pointers", &self.cell_pointers)
            .field("page_size", &self.page_size)
            .field("usable_size", &self.usable_size)
            .finish()
    }
}

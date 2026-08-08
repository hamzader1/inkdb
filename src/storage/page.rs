use std::marker::PhantomData;

use super::btree::CellIdx;
use super::cell::{BTreeCell, IndexInteriorCell, IndexLeafCell, TableInteriorCell, TableLeafCell};
use super::records::{Record, RecordMetadata, Value};
use super::sqlite_cursor::SqliteCursor;
use crate::errors::SqliteError;
use crate::format::page::PageNo;
use crate::pager::guard::PageGuard;
use crate::pager::pager::Pager;
use crate::util::{sqlite_assert_one, sqlite_assert_with_corrupt_err};
use crate::vfs::file::SqliteFile;

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
    pub fn parse(bytes: &[u8]) -> Result<Self, SqliteError> {
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
    pub fn right_most_ptr(&self) -> Option<PageNo> {
        self.right_most_ptr
    }
    pub fn header_size(&self) -> u8 {
        if self.page_kind.is_interior() {
            return INTERIOR_BTREE_PAGE_HEADER_SIZE;
        }
        LEAF_BTREE_PAGE_HEADER_SIZE
    }
}

enum DecodeMode {
    Borrowed,
    Owned,
}
pub struct BTreePageRef<'p> {
    header: BTreePageHeader,
    pub bytes: &'p [u8],
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
    // LIMITED FOR NON-OVERFLOW CELLS
    pub fn record_of_cell<'a, P: SqliteFile>(
        &'a self,
        cell_idx: CellIdx,
        pager: &mut Pager<P>,
    ) -> Result<Vec<Value<'a>>, SqliteError> {
        let mut records = Vec::new();
        let cell = self.cell(cell_idx)?;
        self.get_cell_record(pager, &cell, &mut records)?;
        Ok(records)
    }
    pub fn record_of<'a, P: SqliteFile>(
        &'a self,
        cell: &BTreeCell,
        pager: &mut Pager<P>,
    ) -> Result<Vec<Value<'a>>, SqliteError> {
        let mut records = Vec::new();
        self.get_cell_record(pager, cell, &mut records)?;
        Ok(records)
    }
    /*
           pager: &mut Pager<P>,
       local_payload_bytes: &[u8],
       total_payload_length: usize,
       usable_size: usize,
       first_overflow_page: PageNo,
       current_page_no: PageNo,

    */
    fn get_cell_record<'a, P: SqliteFile>(
        &'a self,
        pager: &mut Pager<P>,
        cell: &BTreeCell,
        collector: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        let bytes: &[u8];
        let mut vec: Option<Vec<u8>> = None;
        if let Some(overflow_page) = cell.overflow_page() {
            let vec = OverflowPageRef::get_total_payload(
                pager,
                self.bytes,
                cell.cell_payload_len() as usize,
                self.usable_size,
                overflow_page,
            )?;
            return self.decode_loop_owned(vec, collector);
        } else {
            return self.decode_loop_borrowed(self.bytes, collector);
        }

        Ok(())
    }
    fn decode_loop_owned<'a>(
        &self,
        bytes: Vec<u8>,
        collector: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        // let bytes = bytes.as_ref();
        let mut header_cursor = SqliteCursor::new(bytes.as_slice());
        let (header_size, consumed) = header_cursor.read_next_varint(self.usable_size)?;
        let mut remaining = (header_size as usize) - consumed;
        let mut data_cursor: SqliteCursor = header_cursor.clone_with_offset(header_size)?;
        // the vec holds the bytes
        while remaining > 0 {
            let (serial_type, consumed) = header_cursor.read_next_varint(self.usable_size)?;
            let record_metadata = Record::content_size(serial_type);
            let data = data_cursor.read_to(record_metadata.size as _)?;
            collector.push(Record::decode_sqltype_owned(data, &record_metadata)); // key change
            remaining -= consumed;
        }
        Ok(())
    }
    fn decode_loop_borrowed<'a>(
        &self,
        bytes: &'a [u8],
        collector: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        // let bytes = bytes.as_ref();
        let mut header_cursor = SqliteCursor::new(bytes);
        let (header_size, consumed) = header_cursor.read_next_varint(self.usable_size)?;
        let mut remaining = (header_size as usize) - consumed;
        let mut data_cursor: SqliteCursor = header_cursor.clone_with_offset(header_size)?;
        // the vec holds the bytes
        while remaining > 0 {
            let (serial_type, consumed) = header_cursor.read_next_varint(self.usable_size)?;
            let record_metadata = Record::content_size(serial_type);
            let data = data_cursor.read_to(record_metadata.size as _)?;
            collector.push(Record::decode_sqltype_borrowed(data, &record_metadata));
            remaining -= consumed;
        }
        Ok(())
    }
    pub fn page_type(&self) -> BTreePageType {
        self.header.page_kind
    }
    pub fn header_size(&self) -> u8 {
        self.header.header_size()
    }
    pub fn is_leaf(&self) -> bool {
        self.header.page_kind.is_leaf()
    }

    pub fn is_interior(&self) -> bool {
        self.header.page_kind.is_interior()
    }
    pub fn right_most_ptr(&self) -> Option<PageNo> {
        self.header.right_most_ptr()
    }
    pub fn no_of_cells(&self) -> u16 {
        self.header.no_of_cells
    }
}

pub struct OverflowPageRef<'a> {
    pub next: PageNo,
    pub data: &'a [u8],
}
impl<'a> OverflowPageRef<'a> {
    pub fn new<T: AsRef<[u8]> + ?Sized>(
        bytes: &'a T,
        usable_size: usize,
    ) -> Result<Self, SqliteError> {
        let data = bytes.as_ref();
        sqlite_assert_with_corrupt_err(
            data.len() >= usable_size,
            "not enough bytes in overflow page",
        )?;

        let next_page_buffer = match data[0..4].as_array::<4>() {
            Some(buf) => buf,
            _ => {
                return Err(SqliteError::Corrupt(
                    "Failed to parse next overflow page from overflow page".into(),
                ))
            }
        };
        let next_page = u32::from_be_bytes(*next_page_buffer);
        let data = &data[4..(usable_size)];

        Ok(Self {
            next: next_page,
            data,
        })
    }
}
impl<'a> OverflowPageRef<'a> {
    pub fn get_total_payload<P: SqliteFile>(
        pager: &mut Pager<P>,
        local_payload_bytes: &[u8],
        total_payload_length: usize,
        usable_size: usize,
        first_overflow_page: PageNo,
    ) -> Result<Vec<u8>, SqliteError> {
        let mut remaining = total_payload_length
            .checked_sub(local_payload_bytes.len())
            .ok_or(SqliteError::Corrupt(
                "local payload exceeds total payload length".into(),
            ))?;
        let mut current_page = first_overflow_page;
        let usable_size = usable_size;
        let mut total_collected_payload: Vec<u8> = Vec::new();
        total_collected_payload.copy_from_slice(local_payload_bytes);
        while remaining > 0 {
            let page = pager.get(current_page)?;
            let buffer = page.bytes_as_ref();
            let overflow_page = OverflowPageRef::new(&buffer, usable_size as _)?;
            let bytes_to_read: usize = remaining.min(overflow_page.data.len());
            total_collected_payload.extend_from_slice(&overflow_page.data[..bytes_to_read]);
            remaining -= bytes_to_read;
            if remaining == 0 {
                if overflow_page.next != 0 {
                    return Err(SqliteError::CorruptedPage {
                        page: current_page,
                        reason: "overflow chain continues after payload is complete".into(),
                    });
                }
                break;
            }
            if overflow_page.next == 0 {
                return Err(SqliteError::CorruptedPage {
                    page: current_page,
                    reason: "overflow chain ends before payload is complete".into(),
                });
            }
            current_page = overflow_page.next;
        }

        sqlite_assert_one(
            local_payload_bytes.len() == total_payload_length,
            SqliteError::Corrupt("assembled payload length mismatch".into()),
        )?;

        Ok(total_collected_payload)
    }
}

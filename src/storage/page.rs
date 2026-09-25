use super::btree::CellIndex;
use super::btree::kind::{Cell, HasPayload};
use super::cell::{BTreeCell, IndexInteriorCell, IndexLeafCell, TableInteriorCell, TableLeafCell};
use super::sqlite_cursor::SqliteCursor;
use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::guard::PageGuard;
use crate::pager::pager::PageNo;
use crate::pager::pager::Pager;
use crate::record::tuple::Tuple;
use crate::record::tuple::{decode_sqltype, into_borrowed, into_owned};
use crate::record::{SqlType, Value};
use crate::util::{
    sqlite_assert_one, sqlite_assert_with_corrupt_err, sqlite_assert_with_runtime_err,
};
use crate::varint::encode_varint;
use crate::vfs::Vfs;
use std::marker::PhantomData;

// use super::cell::BTreeCell;
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

pub const LEFT_CHILD_POINTER_SIZE: usize = 4;
pub const OVERFLOW_POINTER_SIZE: usize = 4;

pub const SQLITE3_HEADER_SIZE: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
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

    pub fn try_from_byte(byte: u8) -> SqliteResult<BTreePageType> {
        Self::get(byte).ok_or(SqliteError::InvalidPageType(byte))
    }

    pub fn is_leaf(&self) -> bool {
        matches!(self, Self::LeafIndex | Self::LeafTable)
    }

    pub fn is_interior(&self) -> bool {
        matches!(self, Self::InteriorTable | Self::InteriorIndex)
    }
    pub fn as_byte(&self) -> u8 {
        match self {
            Self::LeafIndex => 0x0a,
            Self::InteriorIndex => 0x02,
            Self::LeafTable => 0x0d,
            Self::InteriorTable => 0x05,
        }
    }
    pub fn header_size(&self) -> u8 {
        match self {
            Self::InteriorIndex | Self::InteriorTable => INTERIOR_BTREE_PAGE_HEADER_SIZE,
            _ => LEAF_BTREE_PAGE_HEADER_SIZE,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum InsertionState {
    Inserted,
    None,
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
        sqlite_assert_with_corrupt_err(data.len() >= usable_size, || {
            "not enough bytes in overflow page".into()
        })?;

        let next_page_buffer = match data[0..4].as_array::<4>() {
            Some(buf) => buf,
            _ => {
                return Err(SqliteError::Corrupt(
                    "Failed to parse next overflow page from overflow page".into(),
                ));
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

#[derive(Debug, Clone, Copy)]
pub struct FreeCell {
    pub starting_offset: u16,
    pub next: u16,
    pub size: u16,
}

impl FreeCell {
    pub fn parse(ptr: u16, bytes: &[u8]) -> SqliteResult<Self> {
        let mut cursor = SqliteCursor::with_offset(bytes, ptr as _)?;
        let next = cursor.read_next_u16()?;
        let size = cursor.read_next_u16()?;
        Ok(Self {
            starting_offset: ptr,
            next,
            size,
        })
    }
}

impl<'a> OverflowPageRef<'a> {
    pub fn get_total_payload<V: crate::vfs::Vfs>(
        pager: &mut Pager<V>,
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
        let mut total_collected_payload: Vec<u8> = Vec::new();
        total_collected_payload.extend_from_slice(local_payload_bytes);
        while remaining > 0 {
            let page = pager.get(current_page)?;
            let buffer = page.bytes();
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
            total_collected_payload.len() == total_payload_length,
            SqliteError::Corrupt("assembled payload length mismatch".into()),
        )?;

        Ok(total_collected_payload)
    }
}

pub struct PageIterator<'r, 'p, V: crate::vfs::Vfs> {
    page: &'r PageRef<'p>,
    pager: &'r mut Pager<V>,
    index: CellIndex,
}

impl<'r, 'p, V: crate::vfs::Vfs> Iterator for PageIterator<'r, 'p, V> {
    type Item = Vec<Value<'static>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.page.no_of_cells().unwrap_or(0) {
            return None;
        }
        if let Ok(record) = self.page.record_of_cell(self.index, self.pager) {
            self.index += 1;
            return Some(record.into_iter().map(|v| v.into_static()).collect());
        }
        None
    }
}

/*
   ** X is U-35 for table btree leaf pages or ((U-12)*64/255)-23 for index pages.
   ** M is always ((U-12)*32/255)-23.
   ** Let K be M+((P-M)%(U-4)).
   ** If P<=X then all P bytes of payload are stored directly
       on the btree page without overflow.

   ** If P>X and K<=X then the first K bytes of P are stored
       on the btree page and the remaining P-K bytes are stored
       on overflow pages.

   ** If P>X and K>X then the first M bytes of P are stored on
       the btree page and the remaining P-M bytes are stored on
       overflow pages.
*/
pub fn compute_table_local_payload_size(usable_size: usize, payload_len: usize) -> usize {
    let u = usable_size;
    let p = payload_len;
    let x = u - 35;
    if p <= x {
        p
    } else {
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + ((p - m) % (u - 4));
        if k <= x { k } else { m }
    }
}
pub fn compute_index_local_payload_size(usable_size: usize, payload_len: usize) -> usize {
    let u = usable_size;
    let p = payload_len;
    let x = ((u - 12) * 64 / 255) - 23;
    if p <= x {
        p
    } else {
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + ((p - m) % (u - 4));
        if k <= x { k } else { m }
    }
}

pub type PageRef<'a> = BTreePage<&'a [u8]>;
pub type PageMut<'a> = BTreePage<&'a mut [u8]>;

#[derive(Debug)]
pub struct BTreePage<B> {
    page_no: PageNo,
    header_offset: u8,
    page_size: usize,
    usable_size: usize,
    bytes: B,
}

impl<B: AsRef<[u8]>> BTreePage<B> {
    pub fn assert_invariants(&self) -> SqliteResult<()> {
        let hdr = self.header_size()? as usize;
        let n = self.no_of_cells()? as usize;
        let cca = self.cell_content_area()? as usize;
        assert!(hdr + 2 * n <= cca, "pointer array overruns content area");
        assert!(cca <= self.usable_size, "content area past usable size");
        let mut spans: Vec<std::ops::Range<usize>> = Vec::with_capacity(n);
        for i in 0..n {
            let p = self.cell_ptr(i as u16)? as usize;
            assert!(
                p >= hdr && p < self.usable_size,
                "cell {} pointer {} outside content area",
                i,
                p
            );
            spans.push(self.cell_span(p as u16)?);
        }
        spans.sort_by_key(|s| s.start);
        for w in spans.windows(2) {
            assert!(
                w[0].end <= w[1].start,
                "cells overlap: {:?} / {:?}",
                w[0],
                w[1]
            );
        }
        if self.page_type()?.is_interior() && n > 0 {
            assert!(
                self.right_most_ptr()?.is_some(),
                "populated interior page without right-most pointer"
            );
        }
        Ok(())
    }
    pub fn page_no(&self) -> PageNo {
        self.page_no
    }
}

impl<B: AsRef<[u8]>> BTreePage<B> {
    pub fn new(
        page_no: PageNo,
        page_size: usize,
        usable_size: usize,
        bytes: B,
    ) -> SqliteResult<Self> {
        let header_offset = if page_no == 1 { 100 } else { 0 };
        let this = BTreePage {
            page_no,
            page_size,
            usable_size,
            header_offset,
            bytes,
        };
        BTreePageType::try_from_byte(this.u8_at(header_offset as _)?)?;
        Ok(this)
    }
    pub fn usable_size(&self) -> usize {
        self.usable_size
    }
    pub fn page_size(&self) -> usize {
        self.page_size
    }
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_ref()
    }
    pub fn u8_at(&self, off: usize) -> SqliteResult<u8> {
        self.with_cursor_read_at(off, |cursor| cursor.read_next_u8())
    }
    pub fn u16_at(&self, off: usize) -> SqliteResult<u16> {
        self.with_cursor_read_at(off, |cursor| cursor.read_next_u16())
    }
    pub fn u32_at(&self, off: usize) -> SqliteResult<u32> {
        self.with_cursor_read_at(off, |cursor| cursor.read_next_u32())
    }
    pub fn with_cursor_read_at<F, R>(&self, off: usize, f: F) -> SqliteResult<R>
    where
        F: FnOnce(&mut SqliteCursor) -> SqliteResult<R>,
    {
        let mut cursor = SqliteCursor::with_offset(self.bytes(), off as _)?;
        f(&mut cursor)
    }
    pub fn with_header_offset(&self, off: usize) -> usize {
        self.header_offset as usize + off
    }
    pub fn header_size(&self) -> SqliteResult<u8> {
        Ok(self.header_offset + self.page_type()?.header_size())
    }
    pub fn page_type(&self) -> SqliteResult<BTreePageType> {
        BTreePageType::try_from_byte(self.u8_at(self.header_offset as _)?)
    }
    pub fn no_of_cells(&self) -> SqliteResult<u16> {
        self.u16_at(self.with_header_offset(CELL_COUNT_OFFSET))
    }
    pub fn cell_content_area(&self) -> SqliteResult<u16> {
        self.u16_at(self.with_header_offset(CELL_CONTENT_AREA_OFFSET))
    }
    fn frag_cnt(&self) -> SqliteResult<u8> {
        self.u8_at(self.with_header_offset(FRAGMENTED_FREE_BYTES_OFFSET))
    }
    pub fn right_most_ptr(&self) -> SqliteResult<Option<u32>> {
        if self.page_type()?.is_interior() {
            return Ok(Some(
                self.u32_at(self.with_header_offset(RIGHT_MOST_POINTER_OFFSET))?,
            ));
        }
        Ok(None)
    }
    pub fn first_freeblock(&self) -> SqliteResult<u16> {
        self.u16_at(self.with_header_offset(FIRST_FREEBLOCK_OFFSET))
    }

    pub fn cell_ptr(&self, i: u16) -> SqliteResult<u16> {
        if i >= self.no_of_cells()? {
            return Err(SqliteError::InvalidCellPointer(i));
        }
        self.u16_at((self.header_size()? as u16 + i * 2) as usize)
    }
    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn cell_ptrs(&self) -> SqliteResult<impl Iterator<Item = u16> + '_> {
        let start = self.header_size()? as usize;
        let no_of_cells = self.no_of_cells()? as usize;
        Ok(self.bytes()[start..start + no_of_cells * 2]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]])))
    }

    pub fn cell(&self, i: u16) -> SqliteResult<BTreeCell> {
        let cell_offset = self.cell_ptr(i)?;
        self.parse_cell_at(cell_offset)
    }
    pub fn is_index(&self) -> SqliteResult<bool> {
        Ok(self.page_type()? == BTreePageType::InteriorIndex
            || self.page_type()? == BTreePageType::LeafIndex)
    }

    pub fn freespace(&self) -> SqliteResult<usize> {
        let freeblocks_size = self.freeblocks_size()?;
        let total_free_bytes =
            freeblocks_size + self.frag_cnt()? as usize + self.cell_content_area()? as usize
                - (self.header_size()? as u16 + self.no_of_cells()? * 2) as usize;
        Ok(total_free_bytes)
    }
    fn freeblocks_size(&self) -> SqliteResult<usize> {
        if self.first_freeblock()? == 0 {
            return Ok(0);
        }
        let mut cursor = SqliteCursor::with_offset(self.bytes(), self.first_freeblock()? as u64)?;
        let mut total_size = 0;
        let mut next_freeblock_offset = cursor.read_next_u16()?;
        let mut freeblock_size = cursor.read_next_u16()?;
        total_size += freeblock_size;
        while next_freeblock_offset != 0 {
            cursor.set_offset(next_freeblock_offset as _);
            next_freeblock_offset = cursor.read_next_u16()?;
            freeblock_size = cursor.read_next_u16()?;
            total_size += freeblock_size;
        }

        Ok(total_size as _)
    }
    pub fn is_underflow(&self) -> SqliteResult<bool> {
        Ok(self.freespace()? > self.usable_size * 2 / 3)
    }
    pub fn record_of_cell<V: crate::vfs::Vfs>(
        &self,
        cell_idx: u16,
        pager: &mut Pager<V>,
    ) -> Result<Vec<Value<'_>>, SqliteError> {
        let mut records = Vec::new();
        let cell = self.cell(cell_idx)?;
        self.get_cell_record(pager, &cell, &mut records)?;
        Ok(records)
    }

    // pub fn record_of_cell_v2<V:Vfs, C>(&self,
    pub fn record_of<V: crate::vfs::Vfs>(
        &self,
        cell: &BTreeCell,
        pager: &mut Pager<V>,
    ) -> Result<Vec<Value<'_>>, SqliteError> {
        let mut records = Vec::new();
        self.get_cell_record(pager, cell, &mut records)?;
        Ok(records)
    }

    pub fn record_of_cell_into<'a, V: crate::vfs::Vfs>(
        &'a self,
        cell_idx: u16,
        pager: &mut Pager<V>,
        records: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        let cell = self.cell(cell_idx)?;
        self.get_cell_record(pager, &cell, records)
    }

    pub fn record_of_into<'a, V: crate::vfs::Vfs>(
        &'a self,
        cell: &BTreeCell,
        pager: &mut Pager<V>,
        records: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        self.get_cell_record(pager, cell, records)
    }

    pub fn get_cell_record_v2<V: Vfs, C>(
        &self,
        cell: &C,
        pager: &mut Pager<V>,
    ) -> SqliteResult<Vec<Value<'_>>>
    where
        C: HasPayload,
    {
        let mut collector = Vec::new();
        let cell_payload = cell.payload_range();
        let ovp = cell.overflow_page();
        if let Some(overflow_page) = ovp {
            let vec = OverflowPageRef::get_total_payload(
                pager,
                &self.bytes()[cell_payload],
                cell.payload_len() as _,
                self.usable_size,
                overflow_page,
            )?;
            self.decode_loop_owned(vec, &mut collector)?;
        } else {
            self.decode_loop_borrowed(&self.bytes()[cell.payload_range()], &mut collector)?;
        }
        Ok(collector)
    }

    fn get_cell_record<'a, V: crate::vfs::Vfs>(
        &'a self,
        pager: &mut Pager<V>,
        cell: &BTreeCell,
        collector: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        if let Some(overflow_page) = cell.overflow_page() {
            let vec = OverflowPageRef::get_total_payload(
                pager,
                &self.bytes()[cell.payload_range()],
                cell.cell_payload_len() as usize,
                self.usable_size,
                overflow_page,
            )?;
            self.decode_loop_owned(vec, collector)
        } else {
            self.decode_loop_borrowed(&self.bytes()[cell.payload_range()], collector)
        }
    }
    fn decode_loop_owned(
        &self,
        bytes: Vec<u8>,
        collector: &mut Vec<Value<'_>>,
    ) -> Result<(), SqliteError> {
        let mut header_cursor = SqliteCursor::new(bytes.as_slice());
        let (header_size, consumed) = header_cursor.read_next_varint(bytes.len())?;
        let mut remaining = (header_size as usize) - consumed;
        let mut data_cursor: SqliteCursor = header_cursor.clone_with_offset(header_size)?;
        while remaining > 0 {
            let (serial_type, consumed) = header_cursor.read_next_varint(bytes.len())?;
            let record_metadata = Tuple::content_size(serial_type);
            let data = data_cursor.read_to(record_metadata.size as _)?;
            let decoded = decode_sqltype(data, &record_metadata);
            collector.push(into_owned(decoded));
            remaining -= consumed;
        }
        Ok(())
    }
    fn decode_loop_borrowed<'a>(
        &self,
        bytes: &'a [u8],
        collector: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        let mut header_cursor = SqliteCursor::new(bytes);
        let (header_size, consumed) = header_cursor.read_next_varint(bytes.len())?;
        let mut remaining = (header_size as usize) - consumed;
        let mut data_cursor: SqliteCursor = header_cursor.clone_with_offset(header_size)?;
        while remaining > 0 {
            let (serial_type, consumed) = header_cursor.read_next_varint(bytes.len())?;
            let record_metadata = Tuple::content_size(serial_type);
            let data = data_cursor.read_to(record_metadata.size as _)?;
            let decoded = decode_sqltype(data, &record_metadata);
            collector.push(into_borrowed(decoded));
            remaining -= consumed;
        }
        Ok(())
    }
    pub fn cell_key<V: Vfs>(
        &self,
        cell: &BTreeCell,
        pager: &mut Pager<V>,
    ) -> SqliteResult<Value<'static>> {
        match cell {
            BTreeCell::TableLeaf(table_leaf) => Ok(table_leaf.row_id.into_sqlite_value()),
            BTreeCell::TableInterior(table_interior) => {
                Ok(table_interior.rowid_boundary.into_sqlite_value())
            }
            BTreeCell::IndexInterior(index_interior) => {
                let record = self.record_of(cell, pager)?;
                Ok(Value::Tuple(record).into_static())
            }
            BTreeCell::IndexLeaf(index_leaf) => {
                let record = self.record_of(cell, pager)?;
                Ok(Value::Tuple(record).into_static())
            }
        }
    }
    pub fn parse_cell_at(&self, cell_ptr: u16) -> Result<BTreeCell, SqliteError> {
        let start = cell_ptr as usize;
        sqlite_assert_with_corrupt_err(
            start >= self.header_size()? as usize && start < self.usable_size,
            || format!("cell pointer {start} outside content area"),
        )?;
        // let limit = self.usable_size - start; // bytes available to this cell
        let bytes = &self.bytes()[start..self.usable_size];
        let mut cell = match self.page_type()? {
            BTreePageType::InteriorTable => {
                TableInteriorCell::parse(bytes, self.usable_size).map(BTreeCell::TableInterior)
            }
            BTreePageType::LeafTable => {
                TableLeafCell::parse(bytes, self.usable_size).map(BTreeCell::TableLeaf)
            }
            BTreePageType::InteriorIndex => {
                IndexInteriorCell::parse(bytes, self.usable_size).map(BTreeCell::IndexInterior)
            }
            BTreePageType::LeafIndex => {
                IndexLeafCell::parse(bytes, self.usable_size).map(BTreeCell::IndexLeaf)
            }
        }?;
        let base = cell_ptr as usize;
        match &mut cell {
            BTreeCell::TableLeaf(c) => {
                c.payload_range.start += base;
                c.payload_range.end += base;
            }
            BTreeCell::IndexLeaf(c) => {
                c.payload_range.start += base;
                c.payload_range.end += base;
            }
            BTreeCell::IndexInterior(c) => {
                c.payload_range.start += base;
                c.payload_range.end += base;
            }
            BTreeCell::TableInterior(_) => {}
        }
        Ok(cell)
    }
    pub fn cell_span(&self, cell_ptr: u16) -> SqliteResult<std::ops::Range<usize>> {
        let start = cell_ptr as usize;
        let cell = self.parse_cell_at(cell_ptr)?;
        let end = match self.page_type()? {
            BTreePageType::LeafTable => cell.with_table_leaf_cell(|c| {
                c.payload_range.end
                    + if cell.overflow_page().is_some() {
                        OVERFLOW_POINTER_SIZE
                    } else {
                        0
                    }
            }),
            BTreePageType::LeafIndex => cell.with_index_leaf_cell(|c| {
                c.payload_range.end
                    + if c.first_overflow_page.is_some() {
                        OVERFLOW_POINTER_SIZE
                    } else {
                        0
                    }
            }),

            BTreePageType::InteriorTable => cell.with_table_interior_cell(|c| {
                start + LEFT_CHILD_POINTER_SIZE + encode_varint(&mut [0u8; 9], c.rowid_boundary)
            }),

            BTreePageType::InteriorIndex => cell.with_index_interior_cell(|c| {
                /* Start to cell.payload.start covers
                 *
                 * LEFT_CHILD_POINTER_SIZE
                 * encode_varint(&mut [0u8; 9], c.payload_len)
                 *
                 */
                c.payload_range.end
                    + if c.first_overflow_page.is_some() {
                        OVERFLOW_POINTER_SIZE
                    } else {
                        0
                    }
            }),
        };
        Ok(start..end)
    }
    pub fn cell_bytes_as_ref(&self, cell_index: u16) -> SqliteResult<&[u8]> {
        let cell_offset = self.cell_ptr(cell_index)?;
        let cell_span = self.cell_span(cell_offset)?;
        Ok(&self.bytes()[cell_span])
    }
    pub fn remaining_space(&self) -> SqliteResult<usize> {
        Ok(self.cell_content_area()? as usize
            - (self.no_of_cells()? * 2) as usize
            - (self.header_size()?) as usize)
    }

    pub fn is_leaf(&self) -> SqliteResult<bool> {
        Ok(self.page_type()?.is_leaf())
    }

    pub fn is_interior(&self) -> SqliteResult<bool> {
        Ok(self.page_type()?.is_interior())
    }
}

// impl<B:&
impl<B: AsRef<[u8]> + AsMut<[u8]>> BTreePage<B> {
    pub fn new_from_raw_bytes(
        page_no: PageNo,
        page_kind: BTreePageType,
        mut bytes: B,
        page_size: usize,
        usable_size: usize,
    ) -> SqliteResult<Self> {
        let header_offset = if page_no == 1 { 100 } else { 0 };
        let bytes_mut = bytes.as_mut();
        bytes_mut[header_offset as usize..(header_offset as usize) + 1]
            .copy_from_slice(&page_kind.as_byte().to_be_bytes());
        let mut page = Self {
            page_no,
            page_size,
            bytes,
            usable_size,
            header_offset,
        };
        page.reset_for_rebuild()?;
        Ok(page)

        // a recycled frame is not guaranteed to be zeroed, so the cell count
        // has to be written out too instead of relying on the old bytes
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        self.bytes.as_mut()
    }

    fn set_u8_at(&mut self, o: usize, v: u8) {
        self.bytes_mut()[o..o + 1].copy_from_slice(&v.to_be_bytes());
    }
    fn set_u16_at(&mut self, o: usize, v: u16) {
        self.bytes_mut()[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn set_u32_at(&mut self, o: usize, v: u32) {
        self.bytes_mut()[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
    pub fn set_page_type(&mut self, page_type: BTreePageType) -> SqliteResult<()> {
        let byte = page_type.as_byte();
        let offset = self
            .downgrade()?
            .with_header_offset(BTREE_TYPE_PAGE_OFFSET as _);
        self.set_u8_at(offset, byte);
        Ok(())
    }
    fn set_no_of_cells(&mut self, n: u16) -> SqliteResult<()> {
        let o = self.downgrade()?.with_header_offset(CELL_COUNT_OFFSET);
        self.set_u16_at(o, n);
        Ok(())
    }
    fn set_cell_content_area(&mut self, n: u16) -> SqliteResult<()> {
        let o = self
            .downgrade()?
            .with_header_offset(CELL_CONTENT_AREA_OFFSET);
        self.set_u16_at(o, n);
        Ok(())
    }
    fn set_frag_cnt(&mut self, n: u8) -> SqliteResult<()> {
        let o = self
            .downgrade()?
            .with_header_offset(FRAGMENTED_FREE_BYTES_OFFSET);
        self.set_u8_at(o, n);
        Ok(())
    }
    fn set_first_freeblock(&mut self, n: u16) -> SqliteResult<()> {
        let o = self.downgrade()?.with_header_offset(FIRST_FREEBLOCK_OFFSET);
        self.set_u16_at(o, n);
        Ok(())
    }
    pub fn set_right_most_ptr(&mut self, r: u32) -> SqliteResult<()> {
        sqlite_assert_with_runtime_err(self.downgrade()?.page_type()?.is_interior(), || {
            "SetRightMostPointer called on a leaf".into()
        })?;
        let o = self
            .downgrade()?
            .with_header_offset(RIGHT_MOST_POINTER_OFFSET);
        self.set_u32_at(o, r);
        Ok(())
    }
    pub fn reset_for_rebuild(&mut self) -> SqliteResult<()> {
        self.set_cell_content_area(self.usable_size as _)?;
        self.set_first_freeblock(0)?;
        self.set_frag_cnt(0)?;
        self.set_no_of_cells(0)?;
        if self.downgrade()?.page_type()?.is_interior() {
            self.set_right_most_ptr(0)?;
        }
        Ok(())
    }

    fn downgrade(&mut self) -> SqliteResult<PageRef<'_>> {
        BTreePage::<&[u8]>::new(
            self.page_no,
            self.page_size,
            self.usable_size,
            &*self.bytes_mut(),
        )
    }
    /*

     * Claim `size` bytes from the freelist (first-fit).
     * - leftover == 0: unlink the whole block.
     * - 0 < leftover < 4: unlink the block, crumbs go to frag_cnt (too small
     * to form a freeblock).
     * - leftover >= 4: carve `size` bytes off the front, the remainder stays
     * a freeblock at [offset + size] with the old next pointer; prev (or
     * the header when taking from the head) is relinked to it.

    */
    pub fn get_freeblock(&mut self, size: u16) -> SqliteResult<Option<u16>> {
        let bytes = self.bytes_mut();
        let page_ref = self.downgrade()?;
        if page_ref.first_freeblock()? == 0 {
            return Ok(None);
        }
        let mut prev: Option<u16> = None;
        let mut current = page_ref.first_freeblock()?;
        while current != 0 {
            let block = FreeCell::parse(current, page_ref.bytes())?;
            if block.size >= size {
                let leftover = block.size - size;
                if leftover < 4 {
                    // Unlink the whole block; crumbs (<4) become fragmentation.
                    match prev {
                        None => {
                            self.set_first_freeblock(block.next)?;
                        }
                        Some(prev_off) => {
                            let prev_off = prev_off as usize;
                            self.bytes_mut()[prev_off..prev_off + 2]
                                .copy_from_slice(&block.next.to_be_bytes());
                        }
                    }
                    if leftover > 0 {
                        let new_frag_cnt =
                            self.downgrade()?.frag_cnt()?.saturating_add(leftover as _);
                        self.set_frag_cnt(new_frag_cnt)?;
                    }
                } else {
                    // Split: caller takes [offset, offset + size), remainder
                    // stays a freeblock at offset + size.
                    let new_off = current + size;
                    let new_off_usize = new_off as usize;
                    self.bytes_mut()[new_off_usize..new_off_usize + 2]
                        .copy_from_slice(&block.next.to_be_bytes());
                    self.bytes_mut()[new_off_usize + 2..new_off_usize + 4]
                        .copy_from_slice(&leftover.to_be_bytes());
                    match prev {
                        None => {
                            self.set_first_freeblock(new_off)?;
                        }
                        Some(prev_off) => {
                            let prev_off = prev_off as usize;
                            self.bytes_mut()[prev_off..prev_off + 2]
                                .copy_from_slice(&new_off.to_be_bytes());
                        }
                    }
                }
                return Ok(Some(block.starting_offset));
            }
            prev = Some(current);
            current = block.next;
        }
        Ok(None)
    }
    pub fn insert_cell<T: AsRef<[u8]>>(
        &mut self,
        content: &T,
        cell_idx: CellIndex,
    ) -> Result<InsertionState, SqliteError> {
        let content = content.as_ref();
        let page = self.downgrade()?;
        let gap = page
            .cell_content_area()?
            .saturating_sub(page.header_size()? as u16 + page.no_of_cells()? * 2)
            as usize;
        if gap >= 2
            && let Some(offset) = self.get_freeblock(content.as_ref().len() as _)?
        {
            self.insert_cell_at(content, offset as usize, cell_idx, false)?;
            // if matches!(result, Ok(InsertionState::Inserted)) {
            //     // self.debug_check_child_pointers();
            // }
            return Ok(InsertionState::Inserted);
        }
        let page = self.downgrade()?;
        // +2: its cell pointer
        if page.remaining_space()? < content.len() + 2 {
            // we need to defrage
            if page.freespace()? >= content.len() + 2 {
                self.defragment()?;
                let result = self.insert_cell(&content, cell_idx)?;
                sqlite_assert_with_internal_err(
                    matches!(result, InsertionState::Inserted),
                    || "Cell does not fite even after defragementation".into(),
                )?;
                return Ok(result);
            }
            return Ok(InsertionState::None); // overflow
        }
        let offset = self.downgrade()?.cell_content_area()? as usize - content.len();
        self.insert_cell_at(content, offset, cell_idx, true)?;
        // if matches!(result, Ok(InsertionState::Inserted)) {
        //     // self.debug_check_child_pointers();
        // }
        Ok(InsertionState::Inserted)
    }
    fn defragment(&mut self) -> SqliteResult<()> {
        let page = self.as_ref()?;
        let mut cells = Vec::new();
        let mut cells_len = Vec::new();
        for i in 0..page.no_of_cells()? {
            let cell = page.cell_bytes_as_ref(i)?;
            cells.extend_from_slice(cell);
            cells_len.push(cell.len());
        }

        self.reset_for_rebuild()?;
        let mut start = 0;
        for (i, &len) in cells_len.iter().enumerate() {
            let bytes = &cells[start..len + start];
            self.insert_cell(&bytes, i as _)?;
            start += len;
        }
        Ok(())
    }

    // fn debug_check_child_pointers(&self) {
    //     let Ok(n) = self.no_of_cells() else {
    //         return;
    //     };
    //     let Ok(page_type) = self.page_type() else {
    //         return;
    //     };
    //     if !page_type.is_interior() {
    //         return;
    //     }
    //     let mut children = Vec::with_capacity(n as usize + 1);
    //     for i in 0..n {
    //         if let Ok(cell) = self.cell(i) {
    //             children.push((i, cell.left_child()));
    //         }
    //     }
    //     if let Ok(Some(rmp)) = self.right_most_ptr() {
    //         if rmp != 0 {
    //             children.push((n, rmp));
    //         }
    //     }
    //     for i in 0..children.len() {
    //         for j in i + 1..children.len() {
    //             if children[i].1 == children[j].1 {
    //                 eprintln!(
    //                     "DUP_CHILD page={} slots={},{} child={}",
    //                     self.page_no(),
    //                     children[i].0,
    //                     children[j].0,
    //                     children[i].1
    //                 );
    //             }
    //         }
    //     }
    // }
    fn insert_cell_at(
        &mut self,
        content: &[u8],
        offset: usize,
        i: u16,
        from_top: bool,
    ) -> SqliteResult<InsertionState> {
        self.bytes_mut()[offset..offset + content.len()].copy_from_slice(content);
        let arr = self.downgrade()?.header_size()? as u16;
        let n = self.downgrade()?.no_of_cells()?;
        let (from, to) = ((arr + 2 * i) as usize, (arr + 2 * n) as usize);
        self.bytes_mut().copy_within(from..to, from + 2);
        self.set_u16_at(from, offset as u16);
        self.set_no_of_cells(n + 1)?;
        if from_top {
            let cca = self.downgrade()?.cell_content_area()? as usize;
            self.set_cell_content_area((cca - content.len()) as _)?;
        }
        Ok(InsertionState::Inserted)
    }
    pub fn copy_data_from(&mut self, other: &Self) -> Result<(), SqliteError> {
        if self.usable_size != other.usable_size || self.bytes_mut().len() < other.usable_size {
            return Err(SqliteError::Internal(
                "copy_data_from between pages with different usable sizes".into(),
            ));
        }
        let (old_ho, new_ho) = (other.header_offset as usize, self.header_offset as usize);
        self.bytes_mut()[new_ho..other.usable_size]
            .copy_from_slice(&other.bytes.as_ref()[old_ho..other.usable_size]);
        let delta = new_ho as isize - old_ho as isize;
        if delta != 0 {
            let arr = self.downgrade()?.header_size()? as usize;
            let n = self.downgrade()?.no_of_cells()?;
            for i in 0..n {
                let p = self.cell_ptr(i)?;
                self.set_u16_at(arr + 2 * i as usize, (p as isize + delta) as u16);
            }
        }
        Ok(())
    }
    pub fn as_ref(&self) -> SqliteResult<PageRef<'_>> {
        BTreePage::<&[u8]>::new(
            self.page_no,
            self.page_size,
            self.usable_size,
            self.bytes.as_ref(),
        )
    }
    pub fn as_mut_view(&mut self) -> SqliteResult<PageRef<'_>> {
        self.downgrade()
    }
    pub fn insert_freeblock(&mut self, offset: usize, size: usize) -> SqliteResult<()> {
        let hdr = self.downgrade()?.header_size()? as usize;
        sqlite_assert_with_corrupt_err(offset >= hdr && offset + size <= self.usable_size, || {
            format!(
                "freeblock [{offset}, {}) outside content area",
                offset + size
            )
        })?;
        debug_assert!(size >= 4, "Freeblock size cannot be less than 4 bytes");

        // CASE [A]: There are no freeblocks yet.
        if self.downgrade()?.first_freeblock()? == 0 {
            self.bytes_mut()[offset..offset + 2].copy_from_slice(&[0, 0]);
            self.bytes_mut()[offset + 2..offset + 4].copy_from_slice(&(size as u16).to_be_bytes());

            let start = self.header_offset as usize + FIRST_FREEBLOCK_OFFSET;
            let end = start + FIRST_FREEBLOCK_SIZE;

            self.bytes_mut()[start..end].copy_from_slice(&(offset as u16).to_be_bytes());

            return Ok(());
        }

        let first_cell = FreeCell::parse(self.downgrade()?.first_freeblock()?, self.bytes_mut())?;

        // NEW CASE: offset comes before the current first freeblock.
        // This must be checked BEFORE anything else, since every later
        // branch assumes [`offset`] only ever increases relative to the
        // node it's being compared against.
        if offset + size <= first_cell.starting_offset as usize {
            let new_end = offset + size;

            // [NEW][FIRST]  (adjacent -> merge)
            if new_end == first_cell.starting_offset as usize {
                self.bytes_mut()[offset..offset + 2]
                    .copy_from_slice(&first_cell.next.to_be_bytes());

                let new_size = size + first_cell.size as usize;
                self.bytes_mut()[offset + 2..offset + 4]
                    .copy_from_slice(&(new_size as u16).to_be_bytes());
            } else {
                // [NEW] ... [FIRST]  (gap -> just link, no merge)
                self.bytes_mut()[offset..offset + 2]
                    .copy_from_slice(&(first_cell.starting_offset).to_be_bytes());

                self.bytes_mut()[offset + 2..offset + 4]
                    .copy_from_slice(&(size as u16).to_be_bytes());
            }

            // Either way NEW becomes the new head of the freelist.
            let start = self.header_offset as usize + FIRST_FREEBLOCK_OFFSET;
            let end = start + FIRST_FREEBLOCK_SIZE;
            self.bytes_mut()[start..end].copy_from_slice(&(offset as u16).to_be_bytes());

            return Ok(());
        }

        // CASE [B]: There is only one freeblock.
        if first_cell.next == 0 {
            // [A][NEW]
            if first_cell.starting_offset as usize + first_cell.size as usize == offset {
                let first_cell_offset = first_cell.starting_offset as usize;

                self.bytes_mut()[first_cell_offset + 2..first_cell_offset + 4]
                    .copy_from_slice(&((first_cell.size as usize + size) as u16).to_be_bytes());

                return Ok(());
            }

            // [A] ... [NEW]
            // (We already handled offset <= first_cell above, so here
            // offset is guaranteed to be strictly after A and non adjacent.)
            self.bytes_mut()[offset..offset + 2].copy_from_slice(&[0, 0]);
            self.bytes_mut()[offset + 2..offset + 4].copy_from_slice(&(size as u16).to_be_bytes());
            let first_cell_offset = first_cell.starting_offset as usize;
            self.bytes_mut()[first_cell_offset..first_cell_offset + 2]
                .copy_from_slice(&(offset as u16).to_be_bytes());

            return Ok(());
        }

        // Multiple freeblocks.
        let mut prev_cell = first_cell;

        while prev_cell.next != 0 {
            let current_cell = FreeCell::parse(prev_cell.next, self.bytes_mut())?;

            let current_cell_offset = current_cell.starting_offset as usize;
            let current_cell_size = current_cell.size as usize;

            let prev_cell_offset = prev_cell.starting_offset as usize;
            let prev_cell_end = prev_cell_offset + prev_cell.size as usize;

            let new_end = offset + size;

            // [PREV][NEW][CURRENT]
            if prev_cell_end == offset && new_end == current_cell_offset {
                let new_size = prev_cell.size as usize + size + current_cell_size;

                // PREV.size = PREV + NEW + CURRENT
                self.bytes_mut()[prev_cell_offset + 2..prev_cell_offset + 4]
                    .copy_from_slice(&(new_size as u16).to_be_bytes());

                // PREV.next = CURRENT.next
                self.bytes_mut()[prev_cell_offset..prev_cell_offset + 2]
                    .copy_from_slice(&current_cell.next.to_be_bytes());

                return Ok(());
            }

            // [PREV][NEW] ... [CURRENT]
            if prev_cell_end == offset {
                let new_size = prev_cell.size as usize + size;

                self.bytes_mut()[prev_cell_offset + 2..prev_cell_offset + 4]
                    .copy_from_slice(&(new_size as u16).to_be_bytes());

                return Ok(());
            }

            // [PREV] ... [NEW][CURRENT]
            if new_end == current_cell_offset {
                // NEW.next = CURRENT.next
                self.bytes_mut()[offset..offset + 2]
                    .copy_from_slice(&current_cell.next.to_be_bytes());

                // NEW.size = NEW + CURRENT
                self.bytes_mut()[offset + 2..offset + 4]
                    .copy_from_slice(&((size + current_cell_size) as u16).to_be_bytes());

                // PREV.next = NEW
                self.bytes_mut()[prev_cell_offset..prev_cell_offset + 2]
                    .copy_from_slice(&(offset as u16).to_be_bytes());

                return Ok(());
            }

            // [PREV] ... [NEW] ... [CURRENT]
            // Safe now: we've already ruled out offset <= first_cell up
            // front, and by loop/list invariant offset > prev_cell_end
            // whenever we reach this point (nothing between PREV and
            // CURRENT was a match above). We keep the check explicit
            // rather than relying purely on that invariant.
            if offset > prev_cell_end && offset < current_cell_offset {
                // NEW.next = CURRENT
                self.bytes_mut()[offset..offset + 2]
                    .copy_from_slice(&(current_cell_offset as u16).to_be_bytes());

                // NEW.size = size
                self.bytes_mut()[offset + 2..offset + 4]
                    .copy_from_slice(&(size as u16).to_be_bytes());

                // PREV.next = NEW
                self.bytes_mut()[prev_cell_offset..prev_cell_offset + 2]
                    .copy_from_slice(&(offset as u16).to_be_bytes());

                return Ok(());
            }

            prev_cell = current_cell;
        }

        // We reached the last freeblock.
        //
        // [PREV][NEW]
        if prev_cell.starting_offset as usize + prev_cell.size as usize == offset {
            let prev_cell_offset = prev_cell.starting_offset as usize;

            let new_size = prev_cell.size as usize + size;

            self.bytes_mut()[prev_cell_offset + 2..prev_cell_offset + 4]
                .copy_from_slice(&(new_size as u16).to_be_bytes());

            return Ok(());
        }

        // [PREV] ... [NEW]
        self.bytes_mut()[offset..offset + 2].copy_from_slice(&[0, 0]);

        self.bytes_mut()[offset + 2..offset + 4].copy_from_slice(&(size as u16).to_be_bytes());

        let prev_cell_offset = prev_cell.starting_offset as usize;

        self.bytes_mut()[prev_cell_offset..prev_cell_offset + 2]
            .copy_from_slice(&(offset as u16).to_be_bytes());

        Ok(())
    }
    pub fn cell_size(&mut self, i: u16) -> SqliteResult<usize> {
        let s = self.downgrade()?;
        let cell_sp = s.cell_span(s.cell_ptr(i)?)?;
        Ok(cell_sp.end - cell_sp.start)
    }
    pub fn cell_size_by_offset(&mut self, offset: u16) -> SqliteResult<usize> {
        assert!((offset as usize) < self.usable_size);
        let cell_sp = self.downgrade()?.cell_span(offset)?;
        Ok(cell_sp.end - cell_sp.start)
    }
    pub fn remove_cell(&mut self, cell_idx: CellIndex) -> SqliteResult<()> {
        let s = self.downgrade()?;
        let cell_ptr = s.cell_ptr(cell_idx)?;
        let cell_span = s.cell_span(cell_ptr)?;
        let bytes_len: usize = cell_span.end - cell_span.start;
        let n = s.no_of_cells()?;
        self.insert_freeblock(cell_ptr as _, bytes_len)?;
        self.remove_cell_pointer_entry(cell_idx)?;
        self.set_no_of_cells(n - 1)?;
        Ok(())
    }
    fn remove_cell_pointer_entry(&mut self, i: u16) -> SqliteResult<()> {
        let n = self.downgrade()?.no_of_cells()? as usize;
        debug_assert!(n > i as usize, "Cell index out of the cell pointer array");
        // header_size() already includes header_offset — do not add it again.
        let arr = self.downgrade()?.header_size()? as usize;
        let (from, to) = (arr + 2 * (i as usize + 1), arr + 2 * n);
        self.bytes_mut().copy_within(from..to, from - 2);
        Ok(())
    }
    pub fn replace_cell(
        &mut self,
        i: u16,
        content: impl AsRef<[u8]>,
    ) -> SqliteResult<InsertionState> {
        let content = content.as_ref();
        let n = self.no_of_cells()? as usize;
        if i as usize >= n {
            return Err(SqliteError::Internal(format!(
                "replace_cell: index {i} out of bounds (page holds {n} cells)"
            )));
        }
        // Fit check before touching anything: callers split and retry on
        // None, which is only safe if the page is byte-identical after a
        // refusal. A failed replace must never eat the old cell.
        let mut bodies = content.len();
        for j in 0..n {
            if j != i as usize {
                bodies += self.cell_bytes_as_ref(j as u16)?.len();
            }
        }
        if bodies + n * 2 + self.header_size()? as usize > self.usable_size {
            return Ok(InsertionState::None);
        }
        let old = self.cell_bytes_as_ref(i)?.to_vec();
        self.remove_cell(i)?;
        match self.insert_cell(&content, i)? {
            InsertionState::Inserted => {
                // self.debug_check_child_pointers();
                Ok(InsertionState::Inserted)
            }
            InsertionState::None => {
                // Exact fit was proven above, so this means fragmentation
                // lied. Put the old body back: None must mean untouched.
                match self.insert_cell(&old, i)? {
                    InsertionState::Inserted => Ok(InsertionState::None),
                    _ => Err(SqliteError::Corrupt(
                        "replace_cell: page refused its own old cell".into(),
                    )),
                }
            }
        }
    }
}

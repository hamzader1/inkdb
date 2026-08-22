use std::marker::PhantomData;

use super::btree::CellIdx;
use super::cell::{BTreeCell, IndexInteriorCell, IndexLeafCell, TableInteriorCell, TableLeafCell};
use super::sqlite_cursor::SqliteCursor;
use crate::errors::SqliteError;
use crate::pager::guard::PageGuard;
use crate::pager::pager::PageNo;
use crate::pager::pager::Pager;
use crate::record::{Record, RecordMetadata, Value};
use crate::util::{sqlite_assert_one, sqlite_assert_with_corrupt_err};

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
    pub fn as_byte(&self) -> u8 {
        match self {
            Self::LeafIndex => 0x0a,
            Self::InteriorIndex => 0x02,
            Self::LeafTable => 0x0d,
            Self::InteriorTable => 0x05,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BTreePageHeader {
    pub page_kind: BTreePageType,
    pub first_freeblock: u16,
    pub no_of_cells: u16,
    pub cell_content_area: u16,
    pub frag_cnt: u8,
    pub right_most_ptr: Option<PageNo>,
}
impl BTreePageHeader {
    pub fn new(page_kind: BTreePageType, usable_size: usize) -> Self {
        Self {
            page_kind,
            first_freeblock: 0,
            no_of_cells: 0,
            cell_content_area: usable_size as _,
            frag_cnt: 0,
            right_most_ptr: None,
        }
    }
    pub fn parse(bytes: &[u8], header_offsert: u8) -> Result<Self, SqliteError> {
        let mut cursor = SqliteCursor::with_offset(bytes, header_offsert as _)?;
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

#[derive(Debug, PartialEq)]
pub enum InsertionState {
    Inserted, // we could use Option instead of this enum but just to improve readability
    None,
}

#[derive(Clone)]
pub struct BTreePageRef<'p> {
    pub page_no: PageNo,
    header_offset: u8,
    header: BTreePageHeader,
    pub bytes: &'p [u8],
    page_size: usize,
    usable_size: usize,
    _marker: PhantomData<&'p PageGuard>,
}
impl<'p> BTreePageRef<'p> {
    pub fn new(
        page_no: PageNo,
        bytes: &'p [u8],
        page_size: usize,
        usable_size: usize,
    ) -> Result<Self, SqliteError> {
        let header_offset = if page_no == 1 { 100 } else { 0 };
        let header = BTreePageHeader::parse(bytes, header_offset)?;
        Ok(Self {
            page_no,
            header_offset,
            header,
            bytes,
            page_size,
            usable_size,
            _marker: PhantomData,
        })
    }
    pub fn header(&self) -> BTreePageHeader {
        self.header.to_owned()
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
        self.cell_by_ptr(cell_ptr)
    }

    pub fn cell_by_ptr(&self, cell_ptr: u16) -> Result<BTreeCell, SqliteError> {
        debug_assert!(cell_ptr as usize <= self.usable_size);
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
    pub fn record_of_cell(
        &self,
        cell_idx: CellIdx,
        pager: &mut Pager,
    ) -> Result<Vec<Value<'p>>, SqliteError> {
        let mut records = Vec::new();
        let cell = self.cell(cell_idx)?;
        self.get_cell_record(pager, &cell, &mut records)?;
        Ok(records)
    }
    pub fn record_of(
        &self,
        cell: &BTreeCell,
        pager: &mut Pager,
    ) -> Result<Vec<Value<'p>>, SqliteError> {
        let mut records = Vec::new();
        self.get_cell_record(pager, cell, &mut records)?;
        Ok(records)
    }

    pub fn record_of_cell_into(
        &self,
        cell_idx: CellIdx,
        pager: &mut Pager,
        records: &mut Vec<Value<'p>>,
    ) -> Result<(), SqliteError> {
        let cell = self.cell(cell_idx)?;
        self.get_cell_record(pager, &cell, records)
    }

    pub fn record_of_into(
        &self,
        cell: &BTreeCell,
        pager: &mut Pager,
        records: &mut Vec<Value<'p>>,
    ) -> Result<(), SqliteError> {
        self.get_cell_record(pager, cell, records)
    }
    fn get_cell_record(
        &self,
        pager: &mut Pager,
        cell: &BTreeCell,
        collector: &mut Vec<Value<'p>>,
    ) -> Result<(), SqliteError> {
        if let Some(overflow_page) = cell.overflow_page() {
            let vec = OverflowPageRef::get_total_payload(
                pager,
                &self.bytes[*cell.payload_range()],
                cell.cell_payload_len() as usize,
                self.usable_size,
                overflow_page,
            )?;
            self.decode_loop_owned(vec, collector)
        } else {
            self.decode_loop_borrowed(&self.bytes[*cell.payload_range()], collector)
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
            let record_metadata = Record::content_size(serial_type);
            let data = data_cursor.read_to(record_metadata.size as _)?;
            collector.push(Record::decode_sqltype_owned(data, &record_metadata));
            remaining -= consumed;
        }
        Ok(())
    }
    fn decode_loop_borrowed(
        &self,
        bytes: &'p [u8],
        collector: &mut Vec<Value<'p>>,
    ) -> Result<(), SqliteError> {
        let mut header_cursor = SqliteCursor::new(bytes);
        let (header_size, consumed) = header_cursor.read_next_varint(bytes.len())?;
        let mut remaining = (header_size as usize) - consumed;
        let mut data_cursor: SqliteCursor = header_cursor.clone_with_offset(header_size)?;
        while remaining > 0 {
            let (serial_type, consumed) = header_cursor.read_next_varint(bytes.len())?;
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
        self.header.header_size() + self.header_offset
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

    pub fn iter<'r>(&'r self, pager: &'r mut Pager) -> PageIterator<'r, 'p> {
        PageIterator {
            page: self,
            pager,
            index: 0,
        }
    }
    pub fn records(&self, pager: &mut Pager) -> Result<Vec<Vec<Value<'p>>>, SqliteError> {
        let mut all = Vec::with_capacity(self.no_of_cells() as usize);
        for cell_idx in 0..self.no_of_cells() {
            all.push(self.record_of_cell(cell_idx, pager)?);
        }
        Ok(all)
    }
}

// 'p pager lifetime
pub struct BTreePageMut<'p> {
    pub page_no: PageNo,
    pub header_offset: u8,
    pub header: BTreePageHeader,
    pub bytes: &'p mut [u8],
    page_size: usize,
    usable_size: usize,
    pub cell_pointers: Vec<u16>,
    _marker: PhantomData<&'p PageGuard>,
}
impl<'p> BTreePageMut<'p> {
    pub fn new(
        page_no: PageNo,
        bytes: &'p mut [u8],
        page_size: usize,
        usable_size: usize,
    ) -> Result<Self, SqliteError> {
        let header_offset = if page_no == 1 { 100 } else { 0 };
        let header = BTreePageHeader::parse(bytes, header_offset)?;
        let mut page = BTreePageMut {
            page_no,
            header_offset,
            header,
            bytes,
            page_size,
            usable_size,
            cell_pointers: Vec::new(),
            _marker: PhantomData,
        };
        page.parse_cell_array_into_page()?;
        Ok(page)
    }

    pub fn new_from_scratch(
        page_no: PageNo,
        page_kind: BTreePageType,
        bytes: &'p mut [u8],
        page_size: usize,
        usable_size: usize,
    ) -> Self {
        let new_header = BTreePageHeader::new(page_kind, usable_size);
        let header_offset = if page_no == 1 { 100 } else { 0 };
        let mut page = Self {
            page_no,
            page_size,
            bytes,
            usable_size,
            cell_pointers: Vec::new(),
            header: new_header,
            header_offset,
            _marker: PhantomData,
        };
        page.update_page_kind();
        page.update_cell_content_area();
        // a recycled frame is not guaranteed to be zeroed, so the cell count
        // has to be written out too instead of relying on the old bytes
        page.update_no_of_cells();
        page
    }

    pub fn update_page_kind(&mut self) {
        let byte = self.header.page_kind.as_byte();
        let offset = self.header_offset as usize;
        self.bytes[offset..offset + 1].copy_from_slice(&byte.to_be_bytes());
    }
    fn get_header_size(&self) -> u8 {
        if self.header.page_kind.is_interior() {
            return INTERIOR_BTREE_PAGE_HEADER_SIZE;
        }
        LEAF_BTREE_PAGE_HEADER_SIZE
    }
    fn parse_cell_array_into_page(&mut self) -> Result<(), SqliteError> {
        let header_size = self.get_header_size();
        let Self {
            header_offset,
            header,
            cell_pointers,
            bytes,
            ..
        } = self;
        let mut cursor = SqliteCursor::with_offset(bytes, (*header_offset + header_size) as _)?;
        for _ in 0..header.no_of_cells {
            let cell_pointer = cursor.read_next_u16()?;
            cell_pointers.push(cell_pointer);
        }
        Ok(())
    }

    pub fn update_no_of_cells(&mut self) {
        let cell_cnt = self.header.no_of_cells;
        let offset = CELL_COUNT_OFFSET + self.header_offset as usize;
        self.bytes[offset..offset + CELL_COUNT_SIZE].copy_from_slice(&cell_cnt.to_be_bytes());
    }
    pub fn update_cell_content_area(&mut self) {
        let cca = self.header.cell_content_area;
        let offset = CELL_CONTENT_AREA_OFFSET + self.header_offset as usize;
        self.bytes[offset..offset + CELL_CONTENT_AREA_SIZE].copy_from_slice(&cca.to_be_bytes());
    }

    /// Parses the cell at `cell_ptr` directly from the page bytes.
    ///
    /// Unlike `BTreePageOps::cell_by_ptr` this does not go through
    /// `get_page_as_ref`, so it borrows the page only for the duration of the
    /// call and stays usable while the page is being rebuilt.
    pub fn parse_cell_at(&self, cell_ptr: u16) -> Result<BTreeCell, SqliteError> {
        match self.header.page_kind {
            BTreePageType::InteriorTable => {
                TableInteriorCell::parse(self.bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::TableInterior)
            }
            BTreePageType::LeafTable => {
                TableLeafCell::parse(self.bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::TableLeaf)
            }
            BTreePageType::InteriorIndex => {
                IndexInteriorCell::parse(self.bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::IndexInterior)
            }
            BTreePageType::LeafIndex => {
                IndexLeafCell::parse(self.bytes, cell_ptr, self.usable_size)
                    .map(BTreeCell::IndexLeaf)
            }
        }
    }

    /// The exact byte span the cell occupies on this page.
    ///
    /// Cell pointers are stored in KEY order while the bodies are laid out in
    /// ALLOCATION order, so the distance to the next pointer says nothing about
    /// a cell's length: the span has to be derived from the cell itself.
    pub fn cell_span(&self, cell_ptr: u16) -> Result<std::ops::Range<usize>, SqliteError> {
        let start = cell_ptr as usize;
        let cell = self.parse_cell_at(cell_ptr)?;
        let end = if self.header.page_kind == BTreePageType::InteriorTable {
            // left child pointer + rowid varint, no payload
            start
                + LEFT_CHILD_POINTER_SIZE
                + crate::varint::encode_varint(&mut [0u8; 9], cell.row_id())
        } else {
            // payload_range() covers the LOCAL payload only, the overflow page
            // pointer that follows it belongs to the cell as well
            cell.payload_range().end
                + if cell.overflow_page().is_some() {
                    OVERFLOW_POINTER_SIZE
                } else {
                    0
                }
        };
        debug_assert!(
            end > start && end <= self.usable_size,
            "cell span out of page bounds",
        );
        Ok(start..end)
    }

    /// Drops all cell bookkeeping so the page can be rebuilt from staged cell
    /// bodies. Page kind and right most pointer are preserved.
    ///
    /// We MUST copy out every cell body we still need BEFORE calling
    /// this: the following `insert_cell` calls start allocating at
    /// `usable_size` again and will overwrite the old bodies.
    pub fn reset_for_rebuild(&mut self) {
        self.header.no_of_cells = 0;
        self.header.cell_content_area = self.usable_size as u16;
        self.cell_pointers.clear();
        self.update_no_of_cells();
        self.update_cell_content_area();
    }

    pub fn insert_cell<B: AsRef<[u8]>>(
        &mut self,
        content: &B,
        cell_idx: CellIdx,
    ) -> Result<InsertionState, SqliteError> {
        let content = content.as_ref();
        if self.calculate_free_space() < content.len() + 2 {
            return Ok(InsertionState::None); // overflow
        }
        let entry_offset = self.header.cell_content_area as usize - content.len();
        self.bytes[entry_offset..entry_offset + content.len()].copy_from_slice(content);
        self.cell_pointers.insert(cell_idx as _, entry_offset as _);
        self.header.cell_content_area -= content.len() as u16;
        self.header.no_of_cells += 1;
        self.update_cell_pointers();
        self.update_cell_content_area();
        self.update_no_of_cells();
        Ok(InsertionState::Inserted)
    }

    #[expect(clippy::missing_safety_doc)]
    pub unsafe fn insert_cell_raw(
        &mut self,
        content: *const u8,
        content_len: usize,
        cell_idx: CellIdx,
    ) -> Result<InsertionState, SqliteError> {
        if self.calculate_free_space() < content_len + 2 {
            return Ok(InsertionState::None);
        }
        let entry_offset = self.header.cell_content_area as usize - content_len;
        unsafe {
            std::ptr::copy(
                content,
                self.bytes.as_mut_ptr().add(entry_offset),
                content_len,
            );
        }
        self.cell_pointers.insert(cell_idx as _, entry_offset as _);
        self.header.cell_content_area -= content_len as u16;

        self.header.no_of_cells += 1;
        /*
         * UPDATE INCLUDE:
         *
         * UPDATE CELL POINTERS
         * UPDATE CELL COUNT
         * UPDATE CELL CONTENT ARE
         */

        self.update_metadata(None::<fn()>);
        Ok(InsertionState::Inserted)
    }

    pub fn replace_cell(
        &mut self,
        cell_idx: CellIdx,
        content: impl AsRef<[u8]>,
    ) -> Result<(), SqliteError> {
        let content = content.as_ref();
        if cell_idx as usize >= self.cell_pointers.len() {
            return Err(SqliteError::Corrupt(
                "replace_cell index out of bounds".into(),
            ));
        }

        // cell pointers are in KEY order, not OFFSET order anymore
        // (random inserts allocate each new cell at the lowest offset),
        // so derive each cell's real span from its parsed cell
        let mut cells = Vec::with_capacity(self.cell_pointers.len());
        for i in 0..self.cell_pointers.len() {
            if i == cell_idx as usize {
                cells.push(content.to_vec());
                continue;
            }
            let span = self.cell_span(self.cell_pointers[i])?;
            cells.push(self.bytes[span].to_vec());
        }

        // every body is staged, the page can be rebuilt in place now. this also
        // reclaims the space of the cell being replaced
        self.reset_for_rebuild();
        for (i, cell) in cells.iter().enumerate() {
            if self.insert_cell(cell, i as _)? == InsertionState::None {
                return Err(SqliteError::Corrupt(
                    "replace_cell: replacement does not fit in page".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn update_cell_pointers(&mut self) {
        let mut offset = (self.get_header_size() + self.header_offset) as usize;
        for ptr in &self.cell_pointers {
            self.bytes[offset..offset + 2].copy_from_slice(&ptr.to_be_bytes());
            offset += 2;
        }
    }

    pub fn update_cell_pointers_with(&mut self, cell_pointers: &[u16]) {
        let mut offset = (self.get_header_size() + self.header_offset) as usize;
        for ptr in cell_pointers {
            self.bytes[offset..offset + 2].copy_from_slice(&ptr.to_be_bytes());
            offset += 2;
        }
    }
    pub fn calculate_free_space(&self) -> usize {
        self.header.cell_content_area as usize
            - (self.cell_pointers.len() * 2)
            - (self.header_offset + self.get_header_size()) as usize
    }

    pub fn update_metadata<F>(&mut self, additional_metadata: Option<F>)
    where
        F: FnOnce(),
    {
        // EXPERIMENTAL SINCE WE MAY NEED TO
        // UPDATE SOMETHING INCLUDING THE THREE BELOW
        if let Some(f) = additional_metadata {
            f()
        }
        self.update_cell_content_area();
        self.update_cell_pointers();
        self.update_no_of_cells();
    }
    pub fn update_rmp(&mut self) {
        debug_assert!(
            self.header.page_kind.is_interior(),
            "Leaf pages has no right most pointer"
        );
        debug_assert!(
            self.header.right_most_ptr.is_some(),
            "Right most pointer is not initialiazed yet"
        );
        let offset = RIGHT_MOST_POINTER_OFFSET + self.header_offset as usize;
        self.bytes[offset..offset + RIGHT_MOST_POINTER_SIZE]
            .copy_from_slice(&self.header.right_most_ptr.unwrap().to_be_bytes());
    }

    // UNSAFE TO USE THE HEADER
    // UNSAFE TO CALL UNLESS REWRITE THE HEADER
    pub fn clear(&mut self) {
        self.bytes[self.header_offset as usize..self.usable_size].fill(0);
    }

    pub fn copy_data_from(&mut self, other: &Self) -> Result<(), SqliteError> {
        // cell pointers are absolute page offsets, so a raw copy is only valid
        // between pages that keep their btree header at the same offset
        debug_assert!(
            self.header_offset == other.header_offset,
            "copy_data_from between pages with different header offsets",
        );
        debug_assert!(
            self.usable_size == other.usable_size && self.bytes.len() >= other.usable_size,
            "copy_data_from between pages with different usable sizes",
        );
        self.bytes[..other.usable_size].copy_from_slice(&other.bytes[..other.usable_size]);
        // the cached header/cell pointers described the page BEFORE the copy
        self.header = other.header.clone();
        self.cell_pointers = other.cell_pointers.clone();
        Ok(())
    }

    pub fn get_page_as_ref(&'p self) -> Result<BTreePageRef<'p>, SqliteError> {
        BTreePageRef::new(self.page_no, self.bytes, self.page_size, self.usable_size)
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
impl<'a> OverflowPageRef<'a> {
    pub fn get_total_payload(
        pager: &mut Pager,
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
            total_collected_payload.len() == total_payload_length,
            SqliteError::Corrupt("assembled payload length mismatch".into()),
        )?;

        Ok(total_collected_payload)
    }
}

pub struct PageIterator<'r, 'p> {
    page: &'r BTreePageRef<'p>,
    pager: &'r mut Pager,
    index: CellIdx,
}

impl<'r, 'p> Iterator for PageIterator<'r, 'p> {
    type Item = Vec<Value<'p>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.page.no_of_cells() {
            return None;
        }
        if let Ok(record) = self.page.record_of_cell(self.index, self.pager) {
            self.index += 1;
            return Some(record);
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
pub const fn compute_table_local_payload_size(usable_size: usize, payload_len: usize) -> usize {
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

impl<'p> std::fmt::Debug for BTreePageMut<'p> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BTreePageMut")
            .field("page_no", &self.page_no)
            .field("header_offset", &self.header_offset)
            .field("header", &self.header)
            .field("bytes", &self.bytes.len())
            .field("page_size", &self.page_size)
            .field("usable_size", &self.usable_size)
            .field("cell_pointers", &self.cell_pointers)
            .finish()
    }
}

impl<'p> std::fmt::Debug for BTreePageRef<'p> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BTreePageMut")
            .field("page_no", &self.page_no)
            .field("header_offset", &self.header_offset)
            .field("header", &self.header)
            .field("bytes", &self.bytes.len())
            .field("page_size", &self.page_size)
            .field("usable_size", &self.usable_size)
            .finish()
    }
}

pub trait BTreePageOps<'g> {
    fn cell(&self, cell_idx: CellIdx) -> Result<BTreeCell, SqliteError>;
    fn record_of_cell(
        &'g self,
        cell_idx: CellIdx,
        pager: &mut Pager,
    ) -> Result<Vec<Value<'g>>, SqliteError>;
    fn record_of(
        &'g self,
        cell: &BTreeCell,
        pager: &mut Pager,
    ) -> Result<Vec<Value<'g>>, SqliteError>;
    fn record_of_cell_into(
        &'g self,
        cell_idx: CellIdx,
        pager: &mut Pager,
        records: &mut Vec<Value<'g>>,
    ) -> Result<(), SqliteError>;
    fn record_of_into(
        &'g self,
        cell: &BTreeCell,
        pager: &mut Pager,
        records: &mut Vec<Value<'g>>,
    ) -> Result<(), SqliteError>;

    fn no_of_cells(&self) -> u16;
    fn right_most_ptr(&self) -> Option<PageNo>;
    fn is_interior(&self) -> bool;
    fn is_leaf(&self) -> bool;
    fn header_size(&self) -> u8;
    fn page_type(&self) -> BTreePageType;
    fn cell_by_ptr(&self, cell_offset: u16) -> Result<BTreeCell, SqliteError>;
}

impl<'a> BTreePageOps<'a> for BTreePageMut<'a> {
    fn cell(&self, cell_idx: CellIdx) -> Result<BTreeCell, SqliteError> {
        let page = self.get_page_as_ref()?;
        page.cell(cell_idx)
    }

    fn record_of_cell(
        &'a self,
        cell_idx: CellIdx,
        pager: &mut Pager,
    ) -> Result<Vec<Value<'a>>, SqliteError> {
        let page = self.get_page_as_ref()?;
        page.record_of_cell(cell_idx, pager)
    }
    fn record_of(
        &'a self,
        cell: &BTreeCell,
        pager: &mut Pager,
    ) -> Result<Vec<Value<'a>>, SqliteError> {
        let page = self.get_page_as_ref()?;
        page.record_of(cell, pager)
    }
    fn record_of_cell_into<'b>(
        &'b self,
        cell_idx: CellIdx,
        pager: &mut Pager,
        records: &mut Vec<Value<'b>>,
    ) -> Result<(), SqliteError> {
        let page = self.get_page_as_ref()?;
        let cell = self.cell(cell_idx)?;
        page.get_cell_record(pager, &cell, records)
    }

    fn record_of_into(
        &'a self,
        cell: &BTreeCell,
        pager: &mut Pager,
        records: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        let page = self.get_page_as_ref()?;
        page.get_cell_record(pager, cell, records)
    }
    fn header_size(&self) -> u8 {
        self.get_page_as_ref().unwrap().header_size()
    }
    fn is_interior(&self) -> bool {
        self.get_page_as_ref().unwrap().is_interior()
    }
    fn is_leaf(&self) -> bool {
        self.get_page_as_ref().unwrap().is_leaf()
    }
    fn no_of_cells(&self) -> u16 {
        self.get_page_as_ref().unwrap().no_of_cells()
    }
    fn page_type(&self) -> BTreePageType {
        self.get_page_as_ref().unwrap().page_type()
    }
    fn right_most_ptr(&self) -> Option<PageNo> {
        self.get_page_as_ref().unwrap().right_most_ptr()
    }
    fn cell_by_ptr(&self, cell_offset: u16) -> Result<BTreeCell, SqliteError> {
        let page = self.get_page_as_ref()?;
        page.cell_by_ptr(cell_offset)
    }
}

impl<'a> BTreePageOps<'a> for BTreePageRef<'a> {
    fn cell(&self, cell_idx: CellIdx) -> Result<BTreeCell, SqliteError> {
        self.cell(cell_idx)
    }
    fn record_of(
        &'a self,
        cell: &BTreeCell,
        pager: &mut Pager,
    ) -> Result<Vec<Value<'a>>, SqliteError> {
        self.record_of(cell, pager)
    }
    fn record_of_cell(
        &'a self,
        cell_idx: CellIdx,
        pager: &mut Pager,
    ) -> Result<Vec<Value<'a>>, SqliteError> {
        self.record_of_cell(cell_idx, pager)
    }
    fn record_of_cell_into(
        &'a self,
        cell_idx: CellIdx,
        pager: &mut Pager,
        records: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        self.record_of_cell_into(cell_idx, pager, records)
    }
    fn record_of_into(
        &'a self,
        cell: &BTreeCell,
        pager: &mut Pager,
        records: &mut Vec<Value<'a>>,
    ) -> Result<(), SqliteError> {
        self.record_of_into(cell, pager, records)
    }
    fn header_size(&self) -> u8 {
        self.header_size()
    }
    fn is_interior(&self) -> bool {
        self.is_interior()
    }
    fn is_leaf(&self) -> bool {
        self.is_leaf()
    }
    fn no_of_cells(&self) -> u16 {
        self.no_of_cells()
    }
    fn page_type(&self) -> BTreePageType {
        self.page_type()
    }
    fn right_most_ptr(&self) -> Option<PageNo> {
        self.right_most_ptr()
    }
    fn cell_by_ptr(&self, cell_offset: u16) -> Result<BTreeCell, SqliteError> {
        self.cell_by_ptr(cell_offset)
    }
}

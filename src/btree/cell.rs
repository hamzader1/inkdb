use super::{btree::CellIdx, sqlite_cursor::SqliteCursor};
use crate::errors::SqliteError;
use crate::{compute_local_payload_size, format::page::PageNo};
use std::range::Range;

#[derive(Debug)]
pub enum BTreeCell {
    TableInterior(TableInteriorCell),
    TableLeaf(TableLeafCell),
    IndexInterior(IndexInteriorCell),
    IndexLeaf(IndexLeafCell),
}

#[derive(Debug, PartialEq)]
pub enum BTreeCellType {
    TableInterior,
    TableLeaf,
    IndexInterior,
    IndexLeaf,
}

#[derive(Debug)]
pub struct TableInteriorCell {
    left_child: PageNo,
    rowid_boundary: u64,
}

#[derive(Debug)]
pub struct TableLeafCell {
    payload_len: u64,
    row_id: u64,
    local_payload_range: Range<usize>,
    first_overflow_page: Option<PageNo>,
}
#[derive(Debug)]
pub struct IndexInteriorCell {
    left_child: PageNo,
    payload_len: u64,
    payload: Range<usize>,
    first_overflow_page: Option<PageNo>,
}
#[derive(Debug)]
pub struct IndexLeafCell {
    payload_len: u64,
    payload: Range<usize>,
    first_overflow_page: Option<PageNo>,
}
impl TableInteriorCell {
    pub fn parse(bytes: &[u8], cell_ptr: CellIdx, usable_size: usize) -> Result<Self, SqliteError> {
        let mut cursor = SqliteCursor::with_offset(bytes, cell_ptr as _)?;
        let left_child = cursor.read_next_u32()?;
        if left_child == 0 {
            return Err(SqliteError::Corrupt(
                "invalid left child page number: 0".into(),
            ));
        }
        let (rowid_boundary, _) = cursor.read_next_varint(usable_size)?;
        Ok(Self {
            left_child,
            rowid_boundary,
        })
    }
}
impl TableLeafCell {
    pub fn parse(bytes: &[u8], cell_ptr: CellIdx, usable_size: usize) -> Result<Self, SqliteError> {
        let mut cursor = SqliteCursor::with_offset(bytes, cell_ptr as _)?;
        let (payload_len, _) = cursor.read_next_varint(usable_size)?;
        let (row_id, _) = cursor.read_next_varint(usable_size)?;
        let current_pos = cursor.stream_pos() as usize;
        let local_payload_size = compute_local_payload_size(usable_size, payload_len as usize);
        let local_payload_range = Range::from(current_pos..current_pos + local_payload_size);
        let mut overflow_page: Option<u32> = None;
        if local_payload_size < payload_len as usize {
            cursor.move_forward_by(local_payload_size as _)?;
            let overflow_page_int = cursor.read_next_u32()?;
            if overflow_page_int == 0 {
                return Err(SqliteError::Corrupt("invalid overflow page pointer".into()));
            }
            overflow_page = Some(overflow_page_int)
        }

        let cell = Self {
            payload_len,
            row_id,
            local_payload_range,
            first_overflow_page: overflow_page,
        };
        Ok(cell)
    }

    pub fn payload_range(&self) -> &Range<usize> {
        &self.local_payload_range
    }
}

impl IndexInteriorCell {
    pub fn parse(bytes: &[u8], cell_ptr: CellIdx, usable_size: usize) -> Result<Self, SqliteError> {
        // Page number of left child
        let mut cursor = SqliteCursor::with_offset(bytes, cell_ptr as _)?;
        let left_child = cursor.read_next_u32()?;
        if left_child == 0 {
            // use validate function later
            return Err(SqliteError::Corrupt(
                "invalid left child page number: 0".into(),
            ));
        }
        let (payload_len, _) = cursor.read_next_varint(usable_size)?;
        let current_pos = cursor.stream_pos() as usize;
        let payload_size = compute_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size = Range::from(current_pos..current_pos + payload_size as usize);
        let mut overflow_page: Option<PageNo> = None;
        if payload_size < payload_len as usize {
            cursor.move_forward_by(payload_size as _)?;
            let overflow_page_int = cursor.read_next_u32()?;
            if overflow_page_int == 0 {
                return Err(SqliteError::Corrupt("invalid overflow page pointer".into()));
            }
            overflow_page = Some(overflow_page_int)
        }
        let cell = Self {
            left_child,
            payload_len,
            payload: local_payload_size,
            first_overflow_page: overflow_page,
        };

        Ok(cell)
    }

    pub fn payload_range(&self) -> &Range<usize> {
        &self.payload
    }
}

impl IndexLeafCell {
    pub fn parse(bytes: &[u8], cell_ptr: CellIdx, usable_size: usize) -> Result<Self, SqliteError> {
        let mut cursor = SqliteCursor::with_offset(bytes, cell_ptr as _)?;
        let (payload_len, _) = cursor.read_next_varint(usable_size)?;
        let current_pos = cursor.stream_pos() as usize;
        let payload_size = compute_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size = Range::from(current_pos..current_pos + payload_size as usize);
        let mut overflow_page: Option<PageNo> = None;
        if payload_size < payload_len as usize {
            cursor.move_forward_by(payload_size as _)?;
            let overflow_page_int = cursor.read_next_u32()?;
            if overflow_page_int == 0 {
                return Err(SqliteError::Corrupt("invalid overflow page pointer".into()));
            }
            overflow_page = Some(overflow_page_int)
        }
        let cell = Self {
            payload_len,
            payload: local_payload_size,
            first_overflow_page: overflow_page,
        };

        Ok(cell)
    }
    pub fn payload_range(&self) -> &Range<usize> {
        &self.payload
    }
}

impl BTreeCell {
    pub fn row_id(&self) -> u64 {
        match self {
            BTreeCell::TableInterior(x) => x.rowid_boundary,
            BTreeCell::TableLeaf(x) => x.row_id,
            _ => unreachable!(),
        }
    }
    pub fn payload(&self) -> &Range<usize> {
        match self {
            BTreeCell::IndexInterior(x) => x.payload_range(),
            BTreeCell::IndexLeaf(x) => x.payload_range(),
            BTreeCell::TableLeaf(x) => x.payload_range(),
            _ => unreachable!(), // we never reach here, we check before calling
        }
    }
    pub fn cell_type(&self) -> BTreeCellType {
        match self {
            BTreeCell::IndexInterior(_) => BTreeCellType::IndexInterior,
            BTreeCell::IndexLeaf(_) => BTreeCellType::IndexLeaf,
            BTreeCell::TableLeaf(_) => BTreeCellType::TableLeaf,
            _ => BTreeCellType::TableInterior,
        }
    }

    pub fn overflow_page(&self) -> Option<PageNo> {
        match self {
            BTreeCell::IndexInterior(x) => x.first_overflow_page,
            BTreeCell::IndexLeaf(x) => x.first_overflow_page,
            BTreeCell::TableLeaf(x) => x.first_overflow_page,
            _ => unreachable!(),
        }
    }
    pub fn cell_payload_len(&self) -> u64 {
        match self {
            BTreeCell::IndexInterior(x) => x.payload_len,
            BTreeCell::IndexLeaf(x) => x.payload_len,
            BTreeCell::TableLeaf(x) => x.payload_len,
            _ => unreachable!(),
        }
    }

    pub fn left_child(&self) -> PageNo {
        match self {
            BTreeCell::IndexInterior(x) => x.left_child,
            BTreeCell::TableInterior(x) => x.left_child,
            _ => unreachable!(),
        }
    }
}

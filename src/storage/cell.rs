use super::page::{compute_index_local_payload_size, compute_table_local_payload_size};
use super::{btree::CellIndex, sqlite_cursor::SqliteCursor};
use crate::errors::SqliteError;

use crate::pager::pager::PageNo;

use crate::varint::encode_varint;
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
    pub left_child: PageNo,
    pub rowid_boundary: u64,
}

#[derive(Debug)]
pub struct TableLeafCell {
    pub payload_len: u64,
    pub row_id: u64,
    pub local_payload_range: Range<usize>,
    pub first_overflow_page: Option<PageNo>,
}
#[derive(Debug)]
pub struct IndexInteriorCell {
    pub left_child: PageNo,
    pub payload_len: u64,
    pub payload: Range<usize>,
    pub first_overflow_page: Option<PageNo>,
}
#[derive(Debug)]
pub struct IndexLeafCell {
    pub payload_len: u64,
    pub payload: Range<usize>,
    pub first_overflow_page: Option<PageNo>,
}
impl BTreeCell {
    pub fn with_index_leaf_cell<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&IndexLeafCell) -> Option<R>,
    {
        if let Self::IndexLeaf(x) = self {
            return f(x);
        }
        None
    }

    pub fn with_table_leaf_cell<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&TableLeafCell) -> Option<R>,
    {
        if let Self::TableLeaf(x) = self {
            return f(x);
        }
        None
    }

    pub fn with_table_interior_cell<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&TableInteriorCell) -> Option<R>,
    {
        if let Self::TableInterior(x) = self {
            return f(x);
        }
        None
    }

    pub fn with_index_interior_cell<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&IndexInteriorCell) -> Option<R>,
    {
        if let Self::IndexInterior(x) = self {
            return f(x);
        }
        None
    }
}
// TODO: REMOVE FUCKING OFFSET HANDLING BY THE FUCKING CELL
impl TableInteriorCell {
    pub fn parse(
        bytes: &[u8],
        cell_ptr: CellIndex,
        usable_size: usize,
    ) -> Result<Self, SqliteError> {
        let mut cursor = SqliteCursor::with_offset(bytes, cell_ptr as _)?;
        let left_child = cursor.read_next_u32()?;
        if left_child == 0 {
            return Err(SqliteError::Corrupt(
                "invalid left child page number: 0".into(),
            ));
        }
        let (rowid_boundary, _) = cursor.read_next_varint(usable_size.min(bytes.len()))?;
        Ok(Self {
            left_child,
            rowid_boundary,
        })
    }
}
impl TableLeafCell {
    pub fn parse(
        bytes: &[u8],
        cell_ptr: CellIndex,
        usable_size: usize,
    ) -> Result<Self, SqliteError> {
        let mut cursor = SqliteCursor::with_offset(bytes, cell_ptr as _)?;
        let (payload_len, _) = cursor.read_next_varint(usable_size.min(bytes.len()))?;
        let (row_id, _) = cursor.read_next_varint(usable_size.min(bytes.len()))?;
        let current_pos = cursor.stream_pos() as usize;
        let local_payload_size =
            compute_table_local_payload_size(usable_size, payload_len as usize);
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
    pub fn parse(
        bytes: &[u8],
        cell_ptr: CellIndex,
        usable_size: usize,
    ) -> Result<Self, SqliteError> {
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
        let payload_size = compute_index_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size = Range::from(current_pos..current_pos + payload_size);
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
    pub fn parse(
        bytes: &[u8],
        cell_ptr: CellIndex,
        usable_size: usize,
    ) -> Result<Self, SqliteError> {
        let mut cursor = SqliteCursor::with_offset(bytes, cell_ptr as _)?;
        let (payload_len, _) = cursor.read_next_varint(usable_size)?;
        let current_pos = cursor.stream_pos() as usize;
        let payload_size = compute_index_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size = Range::from(current_pos..current_pos + payload_size);
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
    pub fn payload_range(&self) -> &Range<usize> {
        match self {
            BTreeCell::IndexInterior(x) => x.payload_range(),
            BTreeCell::IndexLeaf(x) => x.payload_range(),
            BTreeCell::TableLeaf(x) => x.payload_range(),
            _ => unreachable!(), // we never reach here, we check before calling
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

#[repr(transparent)]
pub struct Encode;
impl Encode {
    pub fn encode_table_leaf_cell(payload: Vec<u8>, row_id: u32) -> Vec<u8> {
        let mut v = Vec::new();
        let mut buff = [0u8; 9];
        let byte_needed_for_len = encode_varint(&mut buff, payload.len() as _);
        v.extend_from_slice(&buff[..byte_needed_for_len]);
        let byte_needed_for_row_id = encode_varint(&mut buff, row_id as _);
        v.extend_from_slice(&buff[..byte_needed_for_row_id]);
        v.extend_from_slice(&payload);
        v
    }

    pub fn encode_table_interior_cell(page_no: PageNo, row_id: u64) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&page_no.to_be_bytes());
        let mut buff = [0u8; 9];
        let byte_needed_for_row_id = encode_varint(&mut buff, row_id as _);
        v.extend_from_slice(&buff[..byte_needed_for_row_id]);
        v
    }
}

impl From<&BTreeCell> for Vec<u8> {
    fn from(value: &BTreeCell) -> Self {
        match value {
            BTreeCell::TableInterior(c) => {
                Encode::encode_table_interior_cell(c.left_child, c.rowid_boundary)
            }
            _ => todo!("Auto encode is not implemented for other cells yet"),
        }
    }
}

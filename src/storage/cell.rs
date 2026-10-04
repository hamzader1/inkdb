use super::cursor::MemCursor;
use super::page::{compute_index_local_payload_size, compute_table_local_payload_size};
use crate::errors::{CorruptError, InkError};

use crate::pager::pager::PageNo;

use crate::varint::encode_varint;
use std::ops::Range;

#[derive(Debug)]
pub struct TableInteriorCell {
    pub left_child: PageNo,
    pub rowid_boundary: u64,
}

#[derive(Debug)]
pub struct TableLeafCell {
    pub payload_len: u64,
    pub row_id: u64,
    pub payload_range: Range<usize>,
    pub first_overflow_page: Option<PageNo>,
}
#[derive(Debug)]
pub struct IndexInteriorCell {
    pub left_child: PageNo,
    pub payload_len: u64,
    pub payload_range: Range<usize>,
    pub first_overflow_page: Option<PageNo>,
}
#[derive(Debug)]
pub struct IndexLeafCell {
    pub payload_len: u64,
    pub payload_range: Range<usize>,
    pub first_overflow_page: Option<PageNo>,
}
impl TableInteriorCell {
    pub fn parse(bytes: &[u8], _: usize) -> Result<Self, InkError> {
        let mut cursor = MemCursor::new(bytes);
        let left_child = cursor.read_next_u32()?;
        if left_child == 0 {
            return Err(InkError::Corrupt(CorruptError::ZeroChildPointer));
        }
        let (rowid_boundary, _) = cursor.read_next_varint(bytes.len())?;
        Ok(Self {
            left_child,
            rowid_boundary,
        })
    }
}
impl TableLeafCell {
    pub fn parse(bytes: &[u8], usable_size: usize) -> Result<Self, InkError> {
        let mut cursor = MemCursor::new(bytes);
        let (payload_len, _) = cursor.read_next_varint(bytes.len())?;
        let (row_id, _) = cursor.read_next_varint(bytes.len())?;
        let current_pos = cursor.stream_pos() as usize;
        let local_payload_size =
            compute_table_local_payload_size(usable_size, payload_len as usize);
        let local_payload_range = current_pos..current_pos + local_payload_size;
        let mut overflow_page: Option<u32> = None;
        if local_payload_size < payload_len as usize {
            cursor.move_forward_by(local_payload_size as _)?;
            let overflow_page_int = cursor.read_next_u32()?;
            if overflow_page_int == 0 {
                return Err(InkError::Corrupt(CorruptError::InvalidOverflowPointer));
            }
            overflow_page = Some(overflow_page_int)
        }

        let cell = Self {
            payload_len,
            row_id,
            payload_range: local_payload_range,
            first_overflow_page: overflow_page,
        };
        Ok(cell)
    }

    pub fn payload_range(&self) -> &Range<usize> {
        &self.payload_range
    }
}

impl IndexInteriorCell {
    pub fn parse(bytes: &[u8], usable_size: usize) -> Result<Self, InkError> {
        // Page number of left child
        let mut cursor = MemCursor::new(bytes);
        let left_child = cursor.read_next_u32()?;
        if left_child == 0 {
            // use validate function later
            return Err(InkError::Corrupt(CorruptError::ZeroChildPointer));
        }
        // Staged cell bytes can be shorter than a page. Size the varint
        // window by what is actually here, like the table parsers do.
        let (payload_len, _) = cursor.read_next_varint(bytes.len())?;
        let current_pos = cursor.stream_pos() as usize;
        let payload_size = compute_index_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size = current_pos..current_pos + payload_size;
        let mut overflow_page: Option<PageNo> = None;
        if payload_size < payload_len as usize {
            cursor.move_forward_by(payload_size as _)?;
            let overflow_page_int = cursor.read_next_u32()?;
            if overflow_page_int == 0 {
                return Err(InkError::Corrupt(CorruptError::InvalidOverflowPointer));
            }
            overflow_page = Some(overflow_page_int)
        }
        let cell = Self {
            left_child,
            payload_len,
            payload_range: local_payload_size,
            first_overflow_page: overflow_page,
        };

        Ok(cell)
    }

    pub fn payload_range(&self) -> &Range<usize> {
        &self.payload_range
    }
}

impl IndexLeafCell {
    pub fn parse(bytes: &[u8], usable_size: usize) -> Result<Self, InkError> {
        let mut cursor = MemCursor::new(bytes);
        // Same short buffer rule as above. Exact cell bytes are often
        // smaller than the page usable size during rebalancing.
        let (payload_len, _) = cursor.read_next_varint(bytes.len())?;
        let current_pos = cursor.stream_pos() as usize;
        let payload_size = compute_index_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size = current_pos..current_pos + payload_size;
        let mut overflow_page: Option<PageNo> = None;
        if payload_size < payload_len as usize {
            cursor.move_forward_by(payload_size as _)?;
            let overflow_page_int = cursor.read_next_u32()?;
            if overflow_page_int == 0 {
                return Err(InkError::Corrupt(CorruptError::InvalidOverflowPointer));
            }
            overflow_page = Some(overflow_page_int)
        }
        let cell = Self {
            payload_len,
            payload_range: local_payload_size,
            first_overflow_page: overflow_page,
        };

        Ok(cell)
    }
    pub fn payload_range(&self) -> &Range<usize> {
        &self.payload_range
    }
}

#[repr(transparent)]
pub struct Encode;
impl Encode {
    pub fn encode_table_leaf_cell(payload: Vec<u8>, row_id: u64) -> Vec<u8> {
        let mut v = Vec::new();
        let mut buff = [0u8; 9];
        let byte_needed_for_len = encode_varint(&mut buff, payload.len() as _);
        v.extend_from_slice(&buff[..byte_needed_for_len]);
        let byte_needed_for_row_id = encode_varint(&mut buff, row_id as _);
        v.extend_from_slice(&buff[..byte_needed_for_row_id]);
        v.extend_from_slice(&payload);
        v
    }

    pub fn encode_index_leaf_cell(record: Vec<u8>) -> Vec<u8> {
        let mut v = Vec::new();
        let mut buff = [0u8; 9];
        let byte_needed_for_len = encode_varint(&mut buff, record.len() as _);
        v.extend_from_slice(&buff[..byte_needed_for_len]);
        v.extend_from_slice(&record);
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

    pub fn encode_index_interior_cell(page_no: PageNo, bytes: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&u32::to_be_bytes(page_no));
        v.extend_from_slice(bytes);
        v
    }
}

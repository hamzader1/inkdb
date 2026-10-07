use super::mem_cursor::MemCursor;
use super::page::{compute_index_local_payload_size, compute_table_local_payload_size};
use crate::errors::{CorruptError, InkError};

use crate::pager::pager::PageNo;

use crate::varint::encode_varint;
use std::ops::Range;

/*
    Reference: https://sqlite.org/fileformat.html#b_tree_pages

    Overview about the structure of a cell

    +-----------+--------------------------------------------------------------+
    | Size      | Description                                                  |
    +-----------+--------------------------------------------------------------+
    | 4         | Page number of the left child. Omitted on leaf page         |
    +-----------+--------------------------------------------------------------+
    | var (1-9) | Number of bytes of data. Omitted on index-tree page or      |
    |           | internal table-tree page                                     |
    +-----------+--------------------------------------------------------------+
    | var (1-9) | Number of bytes of key. Or the key itself for table-tree    |
    |           | page                                                         |
    +-----------+--------------------------------------------------------------+
    | *         | Payload                                                      |
    +-----------+--------------------------------------------------------------+
    | 4         | First page of the overflow chain. Omitted if no overflow    |
    +-----------+--------------------------------------------------------------+

*/

/// An interior cell of a table b-tree.
///
/// It carries the row id that divides the keys around it, and the page number of
/// the subtree holding everything below that row id.
/// # Cell visualisation
/// ```text
///
///      4 bytes         varint
/// +----------------+------------+
/// |   Left child   |   Rowid    |
/// +----------------+------------+
///
/// ```
#[derive(Debug)]
pub struct TableInteriorCell {
    /// The page holding the keys that come before this cell's row id.
    pub left_child: PageNo,
    /// The row id that splits the children of this cell.
    pub rowid_boundary: u64,
}

/// A leaf cell of a table b-tree, which is one row.
///
/// The cell opens with the payload length and the row id, both varints, and then
/// the payload. When the payload is too long to keep on the page, only its start
/// stays here and a pointer to an overflow page follows.
/// # Cell visualisation
/// ```text
///
///      varint       varint          byte array           4 bytes
/// +-------------+------------+----------------------+---------------+
/// | Payload len |   Rowid    |     Payload....      | Overflow page |
/// +-------------+------------+----------------------+---------------+
///
/// ```
#[derive(Debug)]
pub struct TableLeafCell {
    /// The full length of the payload, counting the part kept on overflow pages.
    pub payload_len: u64,
    /// The row id this row is stored under.
    pub row_id: u64,
    /// Where the payload's local part sits in the page.
    pub payload_range: Range<usize>,
    /// The first overflow page, when the payload had to spill.
    pub first_overflow_page: Option<PageNo>,
}
/// An interior cell of an index b-tree.
///
/// It points at a child page and carries an index key. The key is a whole record
/// rather than a row id, so it can be long enough to spill onto overflow pages.
///
/// # Cell visualisation
/// ```text
///
///    4 bytes        varint          byte array           4 bytes
/// +------------+-------------+----------------------+---------------+
/// | Left child | Payload len |     Payload....      | Overflow page |
/// +------------+-------------+----------------------+---------------+
///
/// ```
#[derive(Debug)]
pub struct IndexInteriorCell {
    /// The child page holding the keys that come before this cell's key.
    pub left_child: PageNo,
    /// The full length of the payload, counting the part kept on overflow pages.
    pub payload_len: u64,
    /// Where the payload's local part sits in the page.
    pub payload_range: Range<usize>,
    /// The first overflow page, when the payload had to spill.
    pub first_overflow_page: Option<PageNo>,
}
/// A leaf cell of an index b-tree, which is one index entry.
///
/// It is a payload length followed by the payload, with an overflow pointer when
/// the entry will not fit on the page.
/// // # Cell visualisation
/// ```text
///
///     varint          byte array           4 bytes
/// +-------------+----------------------+---------------+
/// | Payload len |     Payload....      | Overflow page |
/// +-------------+----------------------+---------------+
///
/// ```
#[derive(Debug)]
pub struct IndexLeafCell {
    /// The full length of the payload, counting the part kept on overflow pages.
    pub payload_len: u64,
    /// Where the payload's local part sits in the page.
    pub payload_range: Range<usize>,
    /// The first overflow page, when the payload had to spill.
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

    /// The payload's local part, as a range into the page.
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

    /// The payload's local part, as a range into the page.
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
    /// The payload's local part, as a range into the page.
    pub fn payload_range(&self) -> &Range<usize> {
        &self.payload_range
    }
}

/// The cell encoders, which lay a value out as the bytes a cell takes.
///
/// Nothing is ever built; it is only a place for the four functions to live.
#[repr(transparent)]
pub struct Encode;
impl Encode {
    /// Lay out a table leaf cell: the payload length and the row id as varints, then the payload.
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

    /// Lay out an index leaf cell: the payload length as a varint, then the record.
    pub fn encode_index_leaf_cell(record: Vec<u8>) -> Vec<u8> {
        let mut v = Vec::new();
        let mut buff = [0u8; 9];
        let byte_needed_for_len = encode_varint(&mut buff, record.len() as _);
        v.extend_from_slice(&buff[..byte_needed_for_len]);
        v.extend_from_slice(&record);
        v
    }

    /// Lay out a table interior cell: the child page number in four bytes, then the row id as a varint.
    pub fn encode_table_interior_cell(page_no: PageNo, row_id: u64) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&page_no.to_be_bytes());
        let mut buff = [0u8; 9];
        let byte_needed_for_row_id = encode_varint(&mut buff, row_id as _);
        v.extend_from_slice(&buff[..byte_needed_for_row_id]);
        v
    }

    /// Lay out an index interior cell: the child page number in four bytes, then the key bytes.
    pub fn encode_index_interior_cell(page_no: PageNo, bytes: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&u32::to_be_bytes(page_no));
        v.extend_from_slice(bytes);
        v
    }
}

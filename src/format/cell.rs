use crate::compute_local_payload_size;
use std::io::{
    Read, Seek,
    SeekFrom::{self, Current, Start},
};
use std::range::Range;
type SlotIdx = u16;
type CellOffset = u16;
use crate::bytes::read_u32_be;
use crate::decode_varint;
use crate::errors::SqliteDatabaseError;

const OVERFLOWED_PAGE_SIZE: usize = 4;
use super::page::{CellPointer, PageNumber};

#[derive(Debug)]
pub enum BTreeCell {
    TableInterior(TableInteriorCell),
    TableLeaf(TableLeafCell),
    IndexInterior(IndexInteriorCell),
    IndexLeaf(IndexLeafCell),
}

#[derive(Debug)]
pub struct TableInteriorCell {
    left_child: PageNumber,
    rowid_boundary: u64,
}

#[derive(Debug)]
pub struct TableLeafCell {
    payload_len: u64,
    row_id: u64,
    local_payload_range: Range<usize>,
    first_overflow_page: Option<PageNumber>,
}
#[derive(Debug)]
pub struct IndexInteriorCell {
    left_child: PageNumber,
    payload_len: u64,
    payload: Range<usize>,
    first_overflow_page: Option<PageNumber>,
}
#[derive(Debug)]
pub struct IndexLeafCell {
    payload_len: u64,
    payload: Range<usize>,
    first_overflow_page: Option<PageNumber>,
}
impl TableInteriorCell {
    pub fn parse<R: Read + Seek>(
        r: &mut R,
        cell_ptr: CellPointer,
    ) -> Result<Self, SqliteDatabaseError> {
        r.seek(SeekFrom::Current(cell_ptr.get() as _))?;
        let left_child = read_u32_be(r)?;
        // let rowid_boundary: u64;
        let mut buffer = [0u8; 9];
        r.read_exact(&mut buffer)?;
        let rowid_boundary = match decode_varint(&buffer) {
            Some((rowid_boundary, _)) => rowid_boundary,
            None => return Err(SqliteDatabaseError::InvalidVarint),
        };

        Ok(Self {
            left_child,
            rowid_boundary,
        })
    }
}
// TODO: improve err handling
impl TableLeafCell {
    pub fn parse<R: Read + Seek>(
        r: &mut R,
        cell_ptr: CellPointer,
        usable_size: usize,
    ) -> Result<Self, SqliteDatabaseError> {
        let cell_offset = cell_ptr.get() as u64;
        let mut pre_moved_cursor = r.seek(SeekFrom::Start(cell_offset))?;
        let mut buffer = [0u8; 9];
        r.read_exact(&mut buffer)?;
        let (payload_len, byte_read) = match decode_varint(&mut buffer) {
            Some(x) => x,
            _ => panic!("error while trying to parse payload length"),
        };
        pre_moved_cursor = r.seek(SeekFrom::Start(pre_moved_cursor + byte_read as u64))?; // we are at row_id
        r.read_exact(&mut buffer)?;

        let (row_id, byte_read) = match decode_varint(&mut buffer) {
            Some(x) => x,
            _ => panic!("error while trying to parse payload length"),
        };

        let current_pos = r.seek(SeekFrom::Start(pre_moved_cursor + byte_read as u64))? as usize;
        let local_payload_size = compute_local_payload_size(usable_size, payload_len as usize);
        let local_payload_range = Range::from(current_pos..current_pos + local_payload_size);
        let mut overflow_page: Option<u32> = None;
        if local_payload_size < payload_len as usize {
            r.seek(SeekFrom::Start(
                current_pos as u64 + local_payload_size as u64,
            ))?;
            let overflow_page_int = read_u32_be(r)?;
            assert!(overflow_page_int > 0, "Invalid overflow page pointer");
            overflow_page = Some(read_u32_be(r)?);
        }

        let cell = Self {
            payload_len,
            row_id,
            local_payload_range,
            first_overflow_page: overflow_page,
        };
        Ok(cell)
    }
}
impl IndexInteriorCell {
    pub fn parse<R: Read + Seek>(
        r: &mut R,
        cell_ptr: CellPointer,
        usable_size: usize,
    ) -> Result<Self, SqliteDatabaseError> {
        // Page number of left child
        let cell_header = r.seek(Start(cell_ptr.get() as _))?;
        let left_child = read_u32_be(r)?;
        let mut buf = [0u8; 9];
        r.read_exact(&mut buf)?;
        let (payload_len, byte_read) = match decode_varint(&buf) {
            Some(x) => x,
            _ => return Err(SqliteDatabaseError::InvalidVarint),
        };
        // at the start of the payload
        let pre_moved_cursor = r.seek(Start(cell_header + byte_read as u64))? as usize;
        let payload_size = compute_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size =
            Range::from((pre_moved_cursor..pre_moved_cursor + payload_size as usize));
        let mut overflow_page: Option<PageNumber> = None;
        if payload_size < payload_len as usize {
            r.seek(SeekFrom::Current(payload_size as i64))?;
            let overflow_page_int = read_u32_be(r)?;
            assert!(overflow_page_int > 0, "Invalid overflow page pointer");
            overflow_page = Some(read_u32_be(r)?);
        }
        let cell = Self {
            left_child,
            payload_len,
            payload: local_payload_size,
            first_overflow_page: overflow_page,
        };

        // let overflow
        Ok(cell)
    }
}

// Almost the same as `[IndexInteriorCell]`
// we may create one struct that works for both, but to keep thing organized
// we will keep it as this for now at least.
impl IndexLeafCell {
    pub fn parse<R: Read + Seek>(
        r: &mut R,
        cell_ptr: CellPointer,
        usable_size: usize,
    ) -> Result<Self, SqliteDatabaseError> {
        let cell_header = r.seek(Start(cell_ptr.get() as _))?;
        let mut buf = [0u8; 9];
        r.read_exact(&mut buf)?;
        let (payload_len, byte_read) = match decode_varint(&buf) {
            Some(x) => x,
            _ => return Err(SqliteDatabaseError::InvalidVarint),
        };
        // at the start of the payload
        let pre_moved_cursor = r.seek(Start(cell_header + byte_read as u64))? as usize;
        let payload_size = compute_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size =
            Range::from((pre_moved_cursor..pre_moved_cursor + payload_size as usize));
        let mut overflow_page: Option<PageNumber> = None;
        if payload_size < payload_len as usize {
            r.seek(SeekFrom::Current(payload_size as i64))?;
            let overflow_page_int = read_u32_be(r)?;
            assert!(overflow_page_int > 0, "Invalid overflow page pointer");
            overflow_page = Some(read_u32_be(r)?);
        }
        let cell = Self {
            payload_len,
            payload: local_payload_size,
            first_overflow_page: overflow_page,
        };

        // let overflow
        Ok(cell)
    }
}

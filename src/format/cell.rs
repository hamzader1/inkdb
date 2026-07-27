use std::io::{Read, Seek, SeekFrom};
type SlotIdx = u16;
type CellOffset = u16;
use crate::bytes::read_u32_be;
use crate::decode_varint;
use crate::errors::SqliteDatabaseError;

use super::page::{CellPointer, PageNumber};

#[derive(Debug)]
pub enum BTreeCell {
    TableInterior(TableInteriorCell),
    TableLeaf(TableLeafCell),
    None,
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
impl TableInteriorCell {
    pub fn parse<R: Read + Seek>(
        r: &mut R,
        cell_ptr: CellPointer,
    ) -> Result<Self, SqliteDatabaseError> {
        r.seek(SeekFrom::Current(cell_ptr.get() as _))?;
        let left_child = read_u32_be(r)?;
        // let rowid_boundary: u64;
        let mut buffer = [0u8; 9];
        r.read_exact(&mut buffer);
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

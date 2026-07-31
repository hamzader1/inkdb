use crate::{
    compute_local_payload_size, format::page::BTreePage, util::sqlite_assert_one,
    vfs::file::SqliteFile, DbError, SqliteDatabase,
};
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
use super::{
    page::{CellPointer, PageNumber},
    varint::remaining_varint_bytes,
};

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
        usable_size: usize,
    ) -> Result<Self, SqliteDatabaseError> {
        r.seek(SeekFrom::Current(cell_ptr.get() as _))?;
        let left_child = read_u32_be(r)?;
        // let rowid_boundary: u64;
        let bytes_to_read = remaining_varint_bytes(r, usable_size)?;
        let mut buffer = [0u8; 9];
        r.read_exact(&mut buffer[..bytes_to_read])?;
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
        let bytes_to_read = remaining_varint_bytes(r, usable_size)?;
        let mut buffer = [0u8; 9];
        r.read_exact(&mut buffer[..bytes_to_read])?;
        let (payload_len, byte_read) = match decode_varint(&mut buffer) {
            Some(x) => x,
            _ => return Err(SqliteDatabaseError::InvalidVarint),
        };
        pre_moved_cursor = r.seek(SeekFrom::Start(pre_moved_cursor + byte_read as u64))?; // we are at row_id

        let bytes_to_read = remaining_varint_bytes(r, usable_size)?;
        r.read_exact(&mut buffer[..byte_read])?;

        let (row_id, byte_read) = match decode_varint(&mut buffer) {
            Some(x) => x,
            _ => return Err(SqliteDatabaseError::InvalidVarint),
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
            if overflow_page_int == 0 {
                return Err(SqliteDatabaseError::Corrupt(
                    "invalid overflow page pointer".into(),
                ));
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
    pub fn parse<R: Read + Seek>(
        r: &mut R,
        cell_ptr: CellPointer,
        usable_size: usize,
    ) -> Result<Self, SqliteDatabaseError> {
        // Page number of left child
        let cell_header = r.seek(Start(cell_ptr.get() as _))?;
        let left_child = read_u32_be(r)?;
        let cursor_pos = r.stream_position()? as usize;

        let remaining = usable_size
            .checked_sub(cursor_pos)
            .ok_or(SqliteDatabaseError::InvalidVarint)?;

        let bytes_to_read = remaining_varint_bytes(r, usable_size)?;
        let mut buf = [0u8; 9];
        r.read_exact(&mut buf[..bytes_to_read])?;
        let (payload_len, byte_read) = match decode_varint(&buf) {
            Some(x) => x,
            _ => return Err(SqliteDatabaseError::InvalidVarint),
        };
        // at the start of the payload
        let pre_moved_cursor = r.seek(Start(cell_header + byte_read as u64))? as usize;
        // let payload_ptr = pre_moved_cursor as u16;
        let payload_size = compute_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size =
            Range::from(pre_moved_cursor..pre_moved_cursor + payload_size as usize);
        let mut overflow_page: Option<PageNumber> = None;
        if payload_size < payload_len as usize {
            r.seek(SeekFrom::Current(payload_size as i64))?;
            let overflow_page_int = read_u32_be(r)?;
            if overflow_page_int == 0 {
                return Err(SqliteDatabaseError::Corrupt(
                    "invalid overflow page pointer".into(),
                ));
            }
            overflow_page = Some(overflow_page_int)
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

    pub fn payload_range(&self) -> &Range<usize> {
        &self.payload
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
        let bytes_to_read = remaining_varint_bytes(r, usable_size)?;
        let mut buf = [0u8; 9];
        r.read_exact(&mut buf[..bytes_to_read])?;
        let (payload_len, byte_read) = match decode_varint(&buf) {
            Some(x) => x,
            _ => return Err(SqliteDatabaseError::InvalidVarint),
        };
        // at the start of the payload
        let pre_moved_cursor = r.seek(Start(cell_header + byte_read as u64))? as usize;
        let payload_size = compute_local_payload_size(usable_size, payload_len as usize);
        let local_payload_size =
            Range::from(pre_moved_cursor..pre_moved_cursor + payload_size as usize);
        let mut overflow_page: Option<PageNumber> = None;
        if payload_size < payload_len as usize {
            r.seek(SeekFrom::Current(payload_size as i64))?;
            let overflow_page_int = read_u32_be(r)?;
            if overflow_page_int == 0 {
                return Err(SqliteDatabaseError::Corrupt(
                    "invalid overflow page pointer".into(),
                ));
            }
            overflow_page = Some(overflow_page_int)
        }
        let cell = Self {
            payload_len,
            payload: local_payload_size,
            first_overflow_page: overflow_page,
        };

        // let overflow
        Ok(cell)
    }
    pub fn payload_range(&self) -> &Range<usize> {
        &self.payload
    }
}

impl BTreeCell {
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

    pub fn overflow_page(&self) -> Option<PageNumber> {
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
}

impl<S: SqliteFile> SqliteDatabase<S> {
    pub fn cell_payload(
        &mut self,
        page: &BTreePage,
        cell_idx: u16,
    ) -> Result<Vec<u8>, SqliteDatabaseError> {
        let cell = page.cell(cell_idx)?;
        if cell.cell_type() == BTreeCellType::TableInterior {
            return Err(SqliteDatabaseError::Corrupt(
                "TableInterior has no cells".into(),
            ));
        }
        let local_payload = cell.payload();
        let mut payload = Vec::<u8>::new();
        // let mut cursor = Cursor::new(page.bytes());
        // cursor.seek(SeekFrom::Start( as u64))?;

        payload.extend_from_slice(&page.bytes()[local_payload.start..local_payload.end]);
        if cell.overflow_page().is_none() {
            sqlite_assert_one(
                payload.len() == cell.cell_payload_len() as usize,
                DbError::Corrupt(
                    "payload buffer length does not match original cell payload len".into(),
                ),
            )?;
            return Ok(payload);
        }
        // has overflow
        self.read_overflow_payload(
            payload,
            cell.cell_payload_len() as usize,
            cell.overflow_page().unwrap(),
        )
    }
}

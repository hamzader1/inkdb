//! Shared test infrastructure: an in-memory `SqliteFile` impl plus builders
//! that hand-construct valid (and deliberately corrupt) SQLite bytes so we
//! can unit test parsing logic without touching disk.
//!
//! NOTE: `inkdb::vfs::mem` was empty in the uploaded source, so `MemFile`
//! here is test-only scaffolding, not the crate's real implementation.
//! If/when a real `MemFile` lands in `vfs::mem`, swap this out for it and
//! keep the builder functions below - those are the useful part.

#![allow(dead_code)]

use inkdb::vfs::file::SqliteFile;
use inkdb::DbError;
use std::cell::RefCell;

/// Minimal in-memory file for testing, backed by a growable byte vector.
pub struct MemFile {
    data: RefCell<Vec<u8>>,
}

impl MemFile {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data: RefCell::new(data),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }
}

impl SqliteFile for MemFile {
    fn len(&self) -> Result<u64, DbError> {
        Ok(self.data.borrow().len() as u64)
    }

    fn read_exact_at<B: AsMut<[u8]> + ?Sized>(
        &self,
        offset: u64,
        buff: &mut B,
    ) -> Result<(), DbError> {
        let buff = buff.as_mut();
        let data = self.data.borrow();
        let start = offset as usize;
        let end = start + buff.len();
        if end > data.len() {
            // Mirror what DiskFile would surface for a short read: an IO
            // error mapped through DbError::DatabaseOpenFailure, since
            // SqliteFile has no dedicated "short read" variant.
            return Err(DbError::DatabaseOpenFailure(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "MemFile: read_exact_at out of bounds",
            )));
        }
        buff.copy_from_slice(&data[start..end]);
        Ok(())
    }

    fn write_all_at<B: AsRef<[u8]> + ?Sized>(&self, offset: u64, buff: &B) -> Result<(), DbError> {
        let buff = buff.as_ref();
        let mut data = self.data.borrow_mut();
        let start = offset as usize;
        let end = start + buff.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[start..end].copy_from_slice(buff);
        Ok(())
    }

    fn set_len(&self, len: usize) -> Result<(), DbError> {
        self.data.borrow_mut().resize(len, 0);
        Ok(())
    }

    fn sync(&self) -> Result<(), DbError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Header builders
// ---------------------------------------------------------------------

pub const SQLITE3_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Builds a fully valid 100-byte SQLite database header.
/// `page_size` must be a valid page size encoding (1 for 65536, or a
/// power of two in 512..=32768).
pub fn valid_header_bytes(page_size: u16, reserved_space: u8) -> Vec<u8> {
    let mut h = vec![0u8; 100];
    h[0..16].copy_from_slice(SQLITE3_MAGIC);
    h[16..18].copy_from_slice(&page_size.to_be_bytes());
    h[18] = 1; // file_format_write_version
    h[19] = 1; // file_format_read_version
    h[20] = reserved_space;
    h[21] = 64; // maximum_embedded_payload_fraction
    h[22] = 32; // minimum_embedded_payload_fraction
    h[23] = 32; // leaf_payload_fraction
    h[24..28].copy_from_slice(&0u32.to_be_bytes()); // file_change_counter
    h[28..32].copy_from_slice(&0u32.to_be_bytes()); // database_size_in_pages (filled by caller if needed)
    h[32..36].copy_from_slice(&0u32.to_be_bytes()); // first_freelist_trunk_page
    h[36..40].copy_from_slice(&0u32.to_be_bytes()); // total_number_of_freelist_pages
    h[40..44].copy_from_slice(&0u32.to_be_bytes()); // schema_cookie
    h[44..48].copy_from_slice(&1u32.to_be_bytes()); // schema_format_number
    h[48..52].copy_from_slice(&0u32.to_be_bytes()); // default_page_cache_size
    h[52..56].copy_from_slice(&0u32.to_be_bytes()); // largest_root_btree_page
    h[56..60].copy_from_slice(&1u32.to_be_bytes()); // database_text_encoding (UTF-8)
    h[60..64].copy_from_slice(&0u32.to_be_bytes()); // user_version
    h[64..68].copy_from_slice(&0u32.to_be_bytes()); // incremental_vacuum_mode
    h[68..72].copy_from_slice(&0u32.to_be_bytes()); // application_id
                                                    // 72..92 reserved for expansion -> already zeroed
    h[92..96].copy_from_slice(&0u32.to_be_bytes()); // version_valid_for_number
    h[96..100].copy_from_slice(&0u32.to_be_bytes()); // sqlite_version_number
    h
}

/// Set the database_size_in_pages field (offset 28) on a header buffer.
pub fn set_page_count(header: &mut [u8], count: u32) {
    header[28..32].copy_from_slice(&count.to_be_bytes());
}

pub fn set_freelist(header: &mut [u8], first_trunk: u32, total: u32) {
    header[32..36].copy_from_slice(&first_trunk.to_be_bytes());
    header[36..40].copy_from_slice(&total.to_be_bytes());
}

/// Assemble a full single-page-1 database image: 100-byte header followed
/// by the rest of a page-1 btree page (page 1 embeds the header at offset 0
/// and the btree page header at offset 100).
pub fn build_db_image(
    page_size: usize,
    page_count: u32,
    header_overrides: impl FnOnce(&mut Vec<u8>),
) -> Vec<u8> {
    let mut header = valid_header_bytes(page_size as u16, 0);
    set_page_count(&mut header, page_count);
    header_overrides(&mut header);

    let mut image = vec![0u8; page_size * page_count as usize];
    image[0..100].copy_from_slice(&header);
    image
}

// ---------------------------------------------------------------------
// BTree page builders
// ---------------------------------------------------------------------

pub const LEAF_TABLE: u8 = 0x0d;
pub const INTERIOR_TABLE: u8 = 0x05;
pub const LEAF_INDEX: u8 = 0x0a;
pub const INTERIOR_INDEX: u8 = 0x02;

/// Writes a table-leaf btree page header + cell pointer array + given cells
/// into `page_buf` (which must already be `page_size` long and zeroed).
/// `header_offset` should be 100 for page 1, else 0.
/// `cells` are raw already-encoded cell bytes; they are packed from the end
/// of the page backwards, as SQLite does, and pointers recorded accordingly.
pub fn write_leaf_table_page(page_buf: &mut [u8], header_offset: usize, cells: &[Vec<u8>]) {
    let no_of_cells = cells.len() as u16;

    // Pack cell content from the end of the page backward.
    let mut content_area = page_buf.len();
    let mut pointers = Vec::with_capacity(cells.len());
    for cell in cells {
        content_area -= cell.len();
        page_buf[content_area..content_area + cell.len()].copy_from_slice(cell);
        pointers.push(content_area as u16);
    }

    // Page header (8 bytes for leaf).
    page_buf[header_offset] = LEAF_TABLE;
    page_buf[header_offset + 1..header_offset + 3].copy_from_slice(&0u16.to_be_bytes()); // first_freeblock
    page_buf[header_offset + 3..header_offset + 5].copy_from_slice(&no_of_cells.to_be_bytes());
    page_buf[header_offset + 5..header_offset + 7]
        .copy_from_slice(&(content_area as u16).to_be_bytes());
    page_buf[header_offset + 7] = 0; // frag_cnt

    // Cell pointer array immediately follows the 8-byte header.
    let ptr_array_start = header_offset + 8;
    for (i, ptr) in pointers.iter().enumerate() {
        let off = ptr_array_start + i * 2;
        page_buf[off..off + 2].copy_from_slice(&ptr.to_be_bytes());
    }
}

/// Writes an interior table page (12-byte header) with the given
/// (left_child, rowid_boundary) cells and a right-most pointer.
pub fn write_interior_table_page(
    page_buf: &mut [u8],
    header_offset: usize,
    cells: &[(u32, u64)],
    right_most_ptr: u32,
) {
    // Encode each cell: 4-byte left child + varint rowid boundary.
    let mut encoded_cells = Vec::with_capacity(cells.len());
    for &(left_child, rowid) in cells {
        let mut buf = Vec::new();
        buf.extend_from_slice(&left_child.to_be_bytes());
        let mut varint_buf = [0u8; 9];
        let len = inkdb::encode_varint(&mut varint_buf, rowid);
        buf.extend_from_slice(&varint_buf[..len]);
        encoded_cells.push(buf);
    }

    let no_of_cells = encoded_cells.len() as u16;
    let mut content_area = page_buf.len();
    let mut pointers = Vec::with_capacity(encoded_cells.len());
    for cell in &encoded_cells {
        content_area -= cell.len();
        page_buf[content_area..content_area + cell.len()].copy_from_slice(cell);
        pointers.push(content_area as u16);
    }

    page_buf[header_offset] = INTERIOR_TABLE;
    page_buf[header_offset + 1..header_offset + 3].copy_from_slice(&0u16.to_be_bytes());
    page_buf[header_offset + 3..header_offset + 5].copy_from_slice(&no_of_cells.to_be_bytes());
    page_buf[header_offset + 5..header_offset + 7]
        .copy_from_slice(&(content_area as u16).to_be_bytes());
    page_buf[header_offset + 7] = 0; // frag_cnt
    page_buf[header_offset + 8..header_offset + 12].copy_from_slice(&right_most_ptr.to_be_bytes());

    let ptr_array_start = header_offset + 12;
    for (i, ptr) in pointers.iter().enumerate() {
        let off = ptr_array_start + i * 2;
        page_buf[off..off + 2].copy_from_slice(&ptr.to_be_bytes());
    }
}

/// Encodes a table-leaf cell: varint payload_len, varint row_id, payload
/// bytes, and (if `overflow_page` is Some) a trailing 4-byte overflow
/// pointer. Caller is responsible for making sure `local_payload.len()`
/// matches what `compute_local_payload_size` would compute, if that
/// invariant matters for the test.
pub fn encode_table_leaf_cell(
    payload_len: u64,
    row_id: u64,
    local_payload: &[u8],
    overflow_page: Option<u32>,
) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut vbuf = [0u8; 9];

    let n = inkdb::encode_varint(&mut vbuf, payload_len);
    buf.extend_from_slice(&vbuf[..n]);

    let n = inkdb::encode_varint(&mut vbuf, row_id);
    buf.extend_from_slice(&vbuf[..n]);

    buf.extend_from_slice(local_payload);

    if let Some(p) = overflow_page {
        buf.extend_from_slice(&p.to_be_bytes());
    }
    buf
}

/// Encodes an index-leaf (or index-interior payload portion) cell body:
/// varint payload_len, payload bytes, optional trailing overflow pointer.
pub fn encode_index_leaf_cell(
    payload_len: u64,
    local_payload: &[u8],
    overflow_page: Option<u32>,
) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut vbuf = [0u8; 9];
    let n = inkdb::encode_varint(&mut vbuf, payload_len);
    buf.extend_from_slice(&vbuf[..n]);
    buf.extend_from_slice(local_payload);
    if let Some(p) = overflow_page {
        buf.extend_from_slice(&p.to_be_bytes());
    }
    buf
}

/// Encodes an index-interior cell body: 4-byte left child, then the same
/// layout as `encode_index_leaf_cell`.
pub fn encode_index_interior_cell(
    left_child: u32,
    payload_len: u64,
    local_payload: &[u8],
    overflow_page: Option<u32>,
) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&left_child.to_be_bytes());
    buf.extend_from_slice(&encode_index_leaf_cell(
        payload_len,
        local_payload,
        overflow_page,
    ));
    buf
}

pub fn write_leaf_index_page(page_buf: &mut [u8], header_offset: usize, cells: &[Vec<u8>]) {
    // Layout-identical to leaf-table page (8-byte header), just a
    // different page-kind byte.
    write_leaf_table_page(page_buf, header_offset, cells);
    page_buf[header_offset] = LEAF_INDEX;
}

pub fn write_interior_index_page(
    page_buf: &mut [u8],
    header_offset: usize,
    cells: &[Vec<u8>],
    right_most_ptr: u32,
) {
    let no_of_cells = cells.len() as u16;
    let mut content_area = page_buf.len();
    let mut pointers = Vec::with_capacity(cells.len());
    for cell in cells {
        content_area -= cell.len();
        page_buf[content_area..content_area + cell.len()].copy_from_slice(cell);
        pointers.push(content_area as u16);
    }

    page_buf[header_offset] = INTERIOR_INDEX;
    page_buf[header_offset + 1..header_offset + 3].copy_from_slice(&0u16.to_be_bytes());
    page_buf[header_offset + 3..header_offset + 5].copy_from_slice(&no_of_cells.to_be_bytes());
    page_buf[header_offset + 5..header_offset + 7]
        .copy_from_slice(&(content_area as u16).to_be_bytes());
    page_buf[header_offset + 7] = 0;
    page_buf[header_offset + 8..header_offset + 12].copy_from_slice(&right_most_ptr.to_be_bytes());

    let ptr_array_start = header_offset + 12;
    for (i, ptr) in pointers.iter().enumerate() {
        let off = ptr_array_start + i * 2;
        page_buf[off..off + 2].copy_from_slice(&ptr.to_be_bytes());
    }
}

// ---------------------------------------------------------------------
// Overflow / freelist page builders
// ---------------------------------------------------------------------

/// Builds one overflow page's raw bytes: 4-byte next-page pointer followed
/// by payload bytes, zero padded to `usable_size`.
pub fn build_overflow_page(next: u32, payload_chunk: &[u8], usable_size: usize) -> Vec<u8> {
    let mut buf = vec![0u8; usable_size];
    buf[0..4].copy_from_slice(&next.to_be_bytes());
    let n = payload_chunk.len().min(usable_size - 4);
    buf[4..4 + n].copy_from_slice(&payload_chunk[..n]);
    buf
}

/// Builds a freelist trunk page: next-trunk (4 bytes BE), leaf count
/// (4 bytes BE), then that many leaf page numbers (4 bytes BE each).
pub fn build_freelist_trunk_page(next_trunk: u32, leaf_pages: &[u32], page_size: usize) -> Vec<u8> {
    let mut buf = vec![0u8; page_size];
    buf[0..4].copy_from_slice(&next_trunk.to_be_bytes());
    buf[4..8].copy_from_slice(&(leaf_pages.len() as u32).to_be_bytes());
    for (i, p) in leaf_pages.iter().enumerate() {
        let off = 8 + i * 4;
        buf[off..off + 4].copy_from_slice(&p.to_be_bytes());
    }
    buf
}

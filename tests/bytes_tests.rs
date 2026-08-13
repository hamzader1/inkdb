//! Tests for `inkdb::bytes` helpers (`read_u8`, `read_u16_be`, `read_u32_be`,
//! `read_array`). This module is private (`mod bytes;` in lib.rs, not
//! `pub mod`), so these tests exercise it indirectly through public APIs
//! that call it: `FileCursor::read_next_*`, which are thin wrappers around
//! the exact same big-endian decode logic, and through header/page parsing
//! which is built entirely out of these primitives.

mod common;

use common::MemFile;
use inkdb::vfs::cursor::FileCursor;

#[test]
fn read_next_u8_reads_single_byte_and_advances_offset() {
    let file = MemFile::new(vec![0x7F, 0x01]);
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u8().unwrap(), 0x7F);
    assert_eq!(cur.read_next_u8().unwrap(), 0x01);
}

#[test]
fn read_next_u16_is_big_endian() {
    let file = MemFile::new(vec![0x01, 0x02]);
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u16().unwrap(), 0x0102);
}

#[test]
fn read_next_u32_is_big_endian() {
    let file = MemFile::new(vec![0x00, 0x00, 0x10, 0x00]);
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u32().unwrap(), 0x1000);
}

#[test]
fn read_next_array_reads_exact_length() {
    let file = MemFile::new(b"SQLite format 3\0".to_vec());
    let mut cur = FileCursor::new(&file);
    let arr: [u8; 16] = cur.read_next_array::<16>().unwrap();
    assert_eq!(&arr, b"SQLite format 3\0");
}

#[test]
fn sequential_reads_advance_offset_correctly() {
    // u8 then u16 then u32 back to back, verifying offset bookkeeping.
    let mut data = vec![0xAB];
    data.extend_from_slice(&0x1234u16.to_be_bytes());
    data.extend_from_slice(&0xDEADBEEFu32.to_be_bytes());
    let file = MemFile::new(data);
    let mut cur = FileCursor::new(&file);

    assert_eq!(cur.read_next_u8().unwrap(), 0xAB);
    assert_eq!(cur.read_next_u16().unwrap(), 0x1234);
    assert_eq!(cur.read_next_u32().unwrap(), 0xDEADBEEF);
}

#[test]
fn read_past_end_of_file_errors() {
    let file = MemFile::new(vec![0x01]);
    let mut cur = FileCursor::new(&file);
    assert!(cur.read_next_u32().is_err());
}

#[test]
fn with_offset_starts_at_given_position() {
    let file = MemFile::new(vec![0x00, 0x00, 0x00, 0x42]);
    let mut cur = FileCursor::with_offset(&file, 3);
    assert_eq!(cur.read_next_u8().unwrap(), 0x42);
}

#[test]
fn set_offset_repositions_cursor() {
    let file = MemFile::new(vec![0x11, 0x22, 0x33]);
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u8().unwrap(), 0x11);
    cur.set_offset(0);
    assert_eq!(cur.read_next_u8().unwrap(), 0x11);
    cur.set_offset(2);
    assert_eq!(cur.read_next_u8().unwrap(), 0x33);
}

//! Tests for `inkdb::vfs::cursor::FileCursor`, backed by real `DiskFile`
//! instances (no in-memory `SqliteFile` impl is used, since `vfs::mem` is
//! not implemented yet).

use inkdb::vfs::cursor::FileCursor;
use inkdb::vfs::disk::DiskVfs;
use inkdb::vfs::{SqliteOptions, Vfs};
use std::io::Write;

struct TempFile(std::path::PathBuf);

impl TempFile {
    fn with_contents(name: &str, contents: &[u8]) -> Self {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "inkdb_cursor_disk_test_{}_{}_{}.tmp",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(contents).unwrap();
        f.sync_all().unwrap();
        Self(p)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn open_disk_file(path: &std::path::Path) -> inkdb::vfs::disk::DiskFile {
    let mut vfs = DiskVfs;
    vfs.open(path, SqliteOptions::new().read(true).write(true))
        .unwrap()
}

#[test]
fn cursor_reads_u8_sequentially() {
    let tmp = TempFile::with_contents("u8_seq", &[0x11, 0x22, 0x33]);
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);

    assert_eq!(cur.read_next_u8().unwrap(), 0x11);
    assert_eq!(cur.read_next_u8().unwrap(), 0x22);
    assert_eq!(cur.read_next_u8().unwrap(), 0x33);
}

#[test]
fn cursor_reads_u16_big_endian() {
    let tmp = TempFile::with_contents("u16_be", &[0x01, 0x02]);
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u16().unwrap(), 0x0102);
}

#[test]
fn cursor_reads_u32_big_endian() {
    let tmp = TempFile::with_contents("u32_be", &[0xDE, 0xAD, 0xBE, 0xEF]);
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u32().unwrap(), 0xDEADBEEF);
}

#[test]
fn cursor_reads_fixed_size_array() {
    let tmp = TempFile::with_contents("array", b"SQLite format 3\0");
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);
    let arr: [u8; 16] = cur.read_next_array::<16>().unwrap();
    assert_eq!(&arr, b"SQLite format 3\0");
}

#[test]
fn cursor_with_offset_starts_mid_file() {
    let tmp = TempFile::with_contents("offset_start", b"0123456789");
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::with_offset(&file, 5);
    let mut buf = [0u8; 3];
    cur.read_next_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"567");
}

#[test]
fn cursor_set_offset_repositions() {
    let tmp = TempFile::with_contents("set_offset", b"abcdef");
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u8().unwrap(), b'a');
    cur.set_offset(4);
    assert_eq!(cur.read_next_u8().unwrap(), b'e');
    cur.set_offset(0);
    assert_eq!(cur.read_next_u8().unwrap(), b'a');
}

#[test]
fn cursor_mixed_reads_advance_offset_correctly() {
    let mut data = vec![0xAB]; // u8
    data.extend_from_slice(&0x1234u16.to_be_bytes()); // u16
    data.extend_from_slice(&0xCAFEBABEu32.to_be_bytes()); // u32
    data.extend_from_slice(b"tail!"); // array<5>

    let tmp = TempFile::with_contents("mixed", &data);
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);

    assert_eq!(cur.read_next_u8().unwrap(), 0xAB);
    assert_eq!(cur.read_next_u16().unwrap(), 0x1234);
    assert_eq!(cur.read_next_u32().unwrap(), 0xCAFEBABE);
    let tail: [u8; 5] = cur.read_next_array::<5>().unwrap();
    assert_eq!(&tail, b"tail!");
}

#[test]
fn cursor_read_past_eof_errors() {
    let tmp = TempFile::with_contents("past_eof", &[0x01, 0x02]);
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);
    // Only 2 bytes available, ask for a u32 (4 bytes).
    assert!(cur.read_next_u32().is_err());
}

#[test]
fn cursor_read_exact_at_current_offset_after_partial_read_errors_correctly() {
    let tmp = TempFile::with_contents("partial_then_fail", &[0x01, 0x02, 0x03]);
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);

    assert_eq!(cur.read_next_u8().unwrap(), 0x01);
    // 2 bytes remain, try to read a u32 (4 bytes) -> should fail without
    // panicking, and should ideally not silently succeed with garbage.
    let result = cur.read_next_u32();
    assert!(result.is_err());
}

#[test]
fn two_independent_cursors_over_same_file_do_not_interfere() {
    let tmp = TempFile::with_contents("independent_cursors", b"0123456789");
    let file = open_disk_file(tmp.path());

    let mut cur_a = FileCursor::new(&file);
    let mut cur_b = FileCursor::with_offset(&file, 5);

    assert_eq!(cur_a.read_next_u8().unwrap(), b'0');
    assert_eq!(cur_b.read_next_u8().unwrap(), b'5');
    // cur_a's next read should continue from offset 1, unaffected by cur_b.
    assert_eq!(cur_a.read_next_u8().unwrap(), b'1');
    assert_eq!(cur_b.read_next_u8().unwrap(), b'6');
}

#[test]
fn read_next_exact_with_zero_length_buffer_is_a_no_op() {
    let tmp = TempFile::with_contents("zero_len", b"AB");
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);
    let mut empty: [u8; 0] = [];
    cur.read_next_exact(&mut empty).unwrap();
    // Offset should not have moved.
    assert_eq!(cur.read_next_u8().unwrap(), b'A');
}

#[test]
fn read_next_array_larger_than_remaining_file_errors() {
    let tmp = TempFile::with_contents("array_too_big", &[1, 2, 3]);
    let file = open_disk_file(tmp.path());
    let mut cur = FileCursor::new(&file);
    let result = cur.read_next_array::<10>();
    assert!(result.is_err());
}

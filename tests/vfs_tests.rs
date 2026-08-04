//! Comprehensive tests for `inkdb::vfs` — MemVfs, DiskVfs, SqliteOptions,
//! and FileCursor.

mod common;

use std::io::Write;
use std::rc::Rc;
use std::cell::RefCell;

use inkdb::vfs::cursor::FileCursor;
use inkdb::vfs::disk::{DiskFile, DiskVfs};
use inkdb::vfs::file::SqliteFile;
use inkdb::vfs::mem::{MemFile, MemVfs};
use inkdb::vfs::{SqliteOptions, Vfs};
use inkdb::DbError;

fn rc_vec(data: Vec<u8>) -> Rc<RefCell<Vec<u8>>> {
    Rc::new(RefCell::new(data))
}

// ===========================================================================
// SqliteOptions
// ===========================================================================

#[test]
fn sqlite_options_default_is_read_write() {
    let opts = SqliteOptions::default();
    assert!(opts.can_read());
    assert!(opts.can_write());
    assert!(!opts.is_create());
}

#[test]
fn sqlite_options_new_starts_with_no_flags() {
    let opts = SqliteOptions::new();
    assert!(!opts.can_read());
    assert!(!opts.can_write());
    assert!(!opts.is_create());
}

#[test]
fn sqlite_options_builder_read() {
    let opts = SqliteOptions::new().read(true);
    assert!(opts.can_read());
    assert!(!opts.can_write());
    assert!(!opts.is_create());
}

#[test]
fn sqlite_options_builder_write() {
    let opts = SqliteOptions::new().write(true);
    assert!(!opts.can_read());
    assert!(opts.can_write());
    assert!(!opts.is_create());
}

#[test]
fn sqlite_options_builder_create() {
    let opts = SqliteOptions::new().create(true);
    assert!(!opts.can_read());
    assert!(!opts.can_write());
    assert!(opts.is_create());
}

#[test]
fn sqlite_options_builder_chained() {
    let opts = SqliteOptions::new().read(true).write(true).create(true);
    assert!(opts.can_read());
    assert!(opts.can_write());
    assert!(opts.is_create());
}

#[test]
fn sqlite_options_set_false_clears_flag() {
    let mut opts = SqliteOptions::new().read(true).write(true);
    opts.set(1 << 0, false);
    assert!(!opts.can_read());
    assert!(opts.can_write());
}

#[test]
fn sqlite_options_set_true_sets_flag() {
    let mut opts = SqliteOptions::new();
    opts.set(1 << 1, true);
    assert!(opts.can_write());
}

#[test]
#[should_panic]
fn sqlite_options_set_invalid_flag_panics() {
    let mut opts = SqliteOptions::new();
    opts.set(0xFF, true);
}

// ===========================================================================
// MemVfs
// ===========================================================================

#[test]
fn mem_vfs_open_existing_file_returns_mem_file() {
    let mut vfs = MemVfs::new();
    let data = b"hello world".to_vec();
    vfs.insert("test.db", data.clone());
    let file = vfs.open("test.db", SqliteOptions::default()).unwrap();
    assert_eq!(file.len().unwrap(), data.len() as u64);
}

#[test]
fn mem_vfs_open_nonexistent_file_errors() {
    let mut vfs = MemVfs::new();
    let result = vfs.open("nonexistent.db", SqliteOptions::default());
    assert!(matches!(result, Err(DbError::DatabaseNotExists)));
}

#[test]
fn mem_vfs_open_with_create_flag_still_errors_if_not_inserted() {
    let mut vfs = MemVfs::new();
    let opts = SqliteOptions::new().create(true);
    let result = vfs.open("new.db", opts);
    assert!(matches!(result, Err(DbError::DatabaseNotExists)));
}

#[test]
fn mem_vfs_insert_overwrites_existing() {
    let mut vfs = MemVfs::new();
    vfs.insert("test.db", vec![1, 2, 3]);
    vfs.insert("test.db", vec![4, 5, 6]);
    let file = vfs.open("test.db", SqliteOptions::default()).unwrap();
    let mut buf = [0u8; 3];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [4, 5, 6]);
}

#[test]
fn mem_vfs_multiple_files_are_independent() {
    let mut vfs = MemVfs::new();
    vfs.insert("a.db", vec![1]);
    vfs.insert("b.db", vec![2]);
    let file_a = vfs.open("a.db", SqliteOptions::default()).unwrap();
    let file_b = vfs.open("b.db", SqliteOptions::default()).unwrap();
    let mut buf_a = [0u8; 1];
    let mut buf_b = [0u8; 1];
    file_a.read_exact_at(0, &mut buf_a).unwrap();
    file_b.read_exact_at(0, &mut buf_b).unwrap();
    assert_eq!(buf_a, [1]);
    assert_eq!(buf_b, [2]);
}

// ===========================================================================
// MemFile
// ===========================================================================

#[test]
fn mem_file_len_returns_correct_length() {
    let file = MemFile::new(rc_vec(vec![1, 2, 3, 4, 5]));
    assert_eq!(file.len().unwrap(), 5);
}

#[test]
fn mem_file_len_empty_is_zero() {
    let file = MemFile::new(rc_vec(vec![]));
    assert_eq!(file.len().unwrap(), 0);
}

#[test]
fn mem_file_read_exact_at_reads_correct_bytes() {
    let file = MemFile::new(rc_vec(vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]));
    let mut buf = [0u8; 3];
    file.read_exact_at(3, &mut buf).unwrap();
    assert_eq!(buf, [3, 4, 5]);
}

#[test]
fn mem_file_read_exact_at_offset_zero() {
    let file = MemFile::new(rc_vec(vec![10, 20, 30]));
    let mut buf = [0u8; 3];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [10, 20, 30]);
}

#[test]
fn mem_file_read_exact_at_reads_single_byte() {
    let file = MemFile::new(rc_vec(vec![0xAB]));
    let mut buf = [0u8; 1];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [0xAB]);
}

#[test]
fn mem_file_read_exact_at_past_end_panics() {
    let file = MemFile::new(rc_vec(vec![1, 2, 3]));
    let mut buf = [0u8; 4];
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        file.read_exact_at(0, &mut buf).unwrap();
    }));
    assert!(result.is_err());
}

#[test]
fn mem_file_read_exact_at_offset_past_end_panics() {
    let file = MemFile::new(rc_vec(vec![1, 2, 3]));
    let mut buf = [0u8; 1];
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        file.read_exact_at(5, &mut buf).unwrap();
    }));
    assert!(result.is_err());
}

#[test]
fn mem_file_write_all_at_writes_bytes_at_offset() {
    let file = MemFile::new(rc_vec(vec![0u8; 10]));
    file.write_all_at(3, &[1, 2, 3]).unwrap();
    let mut buf = [0u8; 10];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [0, 0, 0, 1, 2, 3, 0, 0, 0, 0]);
}

#[test]
fn mem_file_write_all_at_offset_zero() {
    let file = MemFile::new(rc_vec(vec![0u8; 5]));
    file.write_all_at(0, &[9, 8, 7]).unwrap();
    let mut buf = [0u8; 5];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [9, 8, 7, 0, 0]);
}

#[test]
fn mem_file_write_all_at_past_end_panics() {
    let file = MemFile::new(rc_vec(vec![0u8; 5]));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        file.write_all_at(4, &[1, 2, 3]).unwrap();
    }));
    assert!(result.is_err());
}

#[test]
fn mem_file_set_len_truncates() {
    let file = MemFile::new(rc_vec(vec![1, 2, 3, 4, 5]));
    file.set_len(3).unwrap();
    assert_eq!(file.len().unwrap(), 3);
}

#[test]
fn mem_file_set_len_extends_with_zeros() {
    let mut data = vec![1u8, 2, 3];
    data.reserve(6);
    let file = MemFile::new(rc_vec(data));
    file.set_len(6).unwrap();
    assert_eq!(file.len().unwrap(), 6);
    let mut buf = [0u8; 6];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [1, 2, 3, 0, 0, 0]);
}

#[test]
fn mem_file_set_len_zero() {
    let file = MemFile::new(rc_vec(vec![1, 2, 3]));
    file.set_len(0).unwrap();
    assert_eq!(file.len().unwrap(), 0);
}

#[test]
fn mem_file_sync_is_noop() {
    let file = MemFile::new(rc_vec(vec![1, 2, 3]));
    file.sync().unwrap();
}

#[test]
fn mem_file_multiple_handles_share_data() {
    let data = rc_vec(vec![1, 2, 3]);
    let file_a = MemFile::new(Rc::clone(&data));
    let file_b = MemFile::new(data);

    file_a.write_all_at(0, &[9, 9, 9]).unwrap();

    let mut buf = [0u8; 3];
    file_b.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [9, 9, 9]);
}

#[test]
fn mem_file_write_then_read_consistent() {
    let file = MemFile::new(rc_vec(vec![0u8; 8]));
    file.write_all_at(2, &[5, 6, 7]).unwrap();
    let mut buf = [0u8; 8];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [0, 0, 5, 6, 7, 0, 0, 0]);
}

// ===========================================================================
// DiskVfs
// ===========================================================================

fn temp_path(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "inkdb_vfs_test_{}_{}.tmp",
        std::process::id(),
        name
    ));
    p
}

fn write_temp_file(path: &std::path::Path, contents: &[u8]) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(contents).unwrap();
    f.sync_all().unwrap();
}

#[test]
fn disk_vfs_open_existing_file_returns_disk_file() {
    let path = temp_path("disk_open_existing");
    write_temp_file(&path, b"hello");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::default()).unwrap();
    assert_eq!(file.len().unwrap(), 5);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_vfs_open_nonexistent_file_without_create_errors() {
    let path = temp_path("disk_open_nonexistent");
    let _ = std::fs::remove_file(&path);
    let mut vfs = DiskVfs;
    let result = vfs.open(&path, SqliteOptions::default());
    assert!(result.is_err());
}

#[test]
fn disk_vfs_open_with_create_creates_file() {
    let path = temp_path("disk_create");
    let _ = std::fs::remove_file(&path);
    let mut vfs = DiskVfs;
    let opts = SqliteOptions::new().create(true).write(true);
    let file = vfs.open(&path, opts).unwrap();
    assert_eq!(file.len().unwrap(), 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_vfs_open_with_create_on_existing_file_succeeds() {
    let path = temp_path("disk_create_existing");
    write_temp_file(&path, b"existing");
    let mut vfs = DiskVfs;
    let opts = SqliteOptions::new().create(true).read(true).write(true);
    let file = vfs.open(&path, opts).unwrap();
    assert_eq!(file.len().unwrap(), 8);
    let _ = std::fs::remove_file(&path);
}

// ===========================================================================
// DiskFile
// ===========================================================================

#[test]
fn disk_file_len_returns_correct_size() {
    let path = temp_path("disk_file_len");
    write_temp_file(&path, &[1, 2, 3, 4, 5]);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::default()).unwrap();
    assert_eq!(file.len().unwrap(), 5);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_file_len_empty_file_is_zero() {
    let path = temp_path("disk_file_empty_len");
    write_temp_file(&path, b"");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::default()).unwrap();
    assert_eq!(file.len().unwrap(), 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_file_read_exact_at_reads_at_offset() {
    let path = temp_path("disk_read_at_offset");
    write_temp_file(&path, b"0123456789");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::default()).unwrap();
    let mut buf = [0u8; 3];
    file.read_exact_at(4, &mut buf).unwrap();
    assert_eq!(buf, *b"456");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_file_read_exact_at_offset_zero() {
    let path = temp_path("disk_read_offset_zero");
    write_temp_file(&path, b"hello");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::default()).unwrap();
    let mut buf = [0u8; 5];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, *b"hello");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_file_read_exact_at_past_end_errors() {
    let path = temp_path("disk_read_past_end");
    write_temp_file(&path, b"hi");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::default()).unwrap();
    let mut buf = [0u8; 5];
    let result = file.read_exact_at(0, &mut buf);
    assert!(result.is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_file_write_all_at_writes_at_offset() {
    let path = temp_path("disk_write_at_offset");
    write_temp_file(&path, &[0u8; 10]);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true).write(true)).unwrap();
    file.write_all_at(3, &[1, 2, 3]).unwrap();
    let mut buf = [0u8; 10];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [0, 0, 0, 1, 2, 3, 0, 0, 0, 0]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_file_set_len_truncates() {
    let path = temp_path("disk_set_len_truncate");
    write_temp_file(&path, &[1, 2, 3, 4, 5]);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true).write(true)).unwrap();
    file.set_len(3).unwrap();
    assert_eq!(file.len().unwrap(), 3);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_file_set_len_extends() {
    let path = temp_path("disk_set_len_extend");
    write_temp_file(&path, &[1, 2, 3]);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true).write(true)).unwrap();
    file.set_len(6).unwrap();
    assert_eq!(file.len().unwrap(), 6);
    let mut buf = [0u8; 6];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf[..3], [1, 2, 3]);
    assert_eq!(buf[3..], [0, 0, 0]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_file_sync_succeeds() {
    let path = temp_path("disk_sync");
    write_temp_file(&path, b"data");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true).write(true)).unwrap();
    file.sync().unwrap();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disk_file_write_then_read_consistent() {
    let path = temp_path("disk_write_then_read");
    write_temp_file(&path, &[0u8; 8]);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true).write(true)).unwrap();
    file.write_all_at(2, &[5, 6, 7]).unwrap();
    let mut buf = [0u8; 8];
    file.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [0, 0, 5, 6, 7, 0, 0, 0]);
    let _ = std::fs::remove_file(&path);
}

// ===========================================================================
// FileCursor with MemFile
// ===========================================================================

#[test]
fn cursor_mem_file_read_u8_sequential() {
    let file = MemFile::new(rc_vec(vec![0x11, 0x22, 0x33]));
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u8().unwrap(), 0x11);
    assert_eq!(cur.read_next_u8().unwrap(), 0x22);
    assert_eq!(cur.read_next_u8().unwrap(), 0x33);
}

#[test]
fn cursor_mem_file_read_u16_big_endian() {
    let file = MemFile::new(rc_vec(vec![0x01, 0x02]));
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u16().unwrap(), 0x0102);
}

#[test]
fn cursor_mem_file_read_u32_big_endian() {
    let file = MemFile::new(rc_vec(vec![0xDE, 0xAD, 0xBE, 0xEF]));
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u32().unwrap(), 0xDEADBEEF);
}

#[test]
fn cursor_mem_file_read_fixed_array() {
    let file = MemFile::new(rc_vec(b"SQLite format 3\0".to_vec()));
    let mut cur = FileCursor::new(&file);
    let arr: [u8; 16] = cur.read_next_arrary::<16>().unwrap();
    assert_eq!(&arr, b"SQLite format 3\0");
}

#[test]
fn cursor_mem_file_with_offset_starts_mid_file() {
    let file = MemFile::new(rc_vec(b"0123456789".to_vec()));
    let mut cur = FileCursor::with_offset(&file, 5);
    let mut buf = [0u8; 3];
    cur.read_next_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"567");
}

#[test]
fn cursor_mem_file_set_offset_repositions() {
    let file = MemFile::new(rc_vec(b"abcdef".to_vec()));
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u8().unwrap(), b'a');
    cur.set_offset(4);
    assert_eq!(cur.read_next_u8().unwrap(), b'e');
    cur.set_offset(0);
    assert_eq!(cur.read_next_u8().unwrap(), b'a');
}

#[test]
fn cursor_mem_file_mixed_reads_advance_offset() {
    let mut data = vec![0xAB];
    data.extend_from_slice(&0x1234u16.to_be_bytes());
    data.extend_from_slice(&0xCAFEBABEu32.to_be_bytes());
    data.extend_from_slice(b"tail!");
    let file = MemFile::new(rc_vec(data));
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u8().unwrap(), 0xAB);
    assert_eq!(cur.read_next_u16().unwrap(), 0x1234);
    assert_eq!(cur.read_next_u32().unwrap(), 0xCAFEBABE);
    let tail: [u8; 5] = cur.read_next_arrary::<5>().unwrap();
    assert_eq!(&tail, b"tail!");
}

#[test]
fn cursor_mem_file_read_past_eof_panics() {
    let file = MemFile::new(rc_vec(vec![0x01, 0x02]));
    let mut cur = FileCursor::new(&file);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cur.read_next_u32().unwrap();
    }));
    assert!(result.is_err());
}

#[test]
fn cursor_mem_file_read_array_larger_than_remaining_panics() {
    let file = MemFile::new(rc_vec(vec![1, 2, 3]));
    let mut cur = FileCursor::new(&file);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cur.read_next_arrary::<10>().unwrap();
    }));
    assert!(result.is_err());
}

#[test]
fn cursor_mem_file_zero_length_buffer_is_noop() {
    let file = MemFile::new(rc_vec(b"AB".to_vec()));
    let mut cur = FileCursor::new(&file);
    let mut empty: [u8; 0] = [];
    cur.read_next_exact(&mut empty).unwrap();
    assert_eq!(cur.read_next_u8().unwrap(), b'A');
}

#[test]
fn cursor_mem_file_two_independent_cursors() {
    let file = MemFile::new(rc_vec(b"0123456789".to_vec()));
    let mut cur_a = FileCursor::new(&file);
    let mut cur_b = FileCursor::with_offset(&file, 5);
    assert_eq!(cur_a.read_next_u8().unwrap(), b'0');
    assert_eq!(cur_b.read_next_u8().unwrap(), b'5');
    assert_eq!(cur_a.read_next_u8().unwrap(), b'1');
    assert_eq!(cur_b.read_next_u8().unwrap(), b'6');
}

// ===========================================================================
// FileCursor with DiskFile
// ===========================================================================

#[test]
fn cursor_disk_file_read_u8_sequential() {
    let path = temp_path("cursor_disk_u8");
    write_temp_file(&path, &[0x11, 0x22, 0x33]);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u8().unwrap(), 0x11);
    assert_eq!(cur.read_next_u8().unwrap(), 0x22);
    assert_eq!(cur.read_next_u8().unwrap(), 0x33);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cursor_disk_file_read_u32_big_endian() {
    let path = temp_path("cursor_disk_u32");
    write_temp_file(&path, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u32().unwrap(), 0xDEADBEEF);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cursor_disk_file_read_fixed_array() {
    let path = temp_path("cursor_disk_array");
    write_temp_file(&path, b"SQLite format 3\0");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::new(&file);
    let arr: [u8; 16] = cur.read_next_arrary::<16>().unwrap();
    assert_eq!(&arr, b"SQLite format 3\0");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cursor_disk_file_with_offset_starts_mid_file() {
    let path = temp_path("cursor_disk_offset");
    write_temp_file(&path, b"0123456789");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::with_offset(&file, 5);
    let mut buf = [0u8; 3];
    cur.read_next_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"567");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cursor_disk_file_set_offset_repositions() {
    let path = temp_path("cursor_disk_set_offset");
    write_temp_file(&path, b"abcdef");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u8().unwrap(), b'a');
    cur.set_offset(4);
    assert_eq!(cur.read_next_u8().unwrap(), b'e');
    cur.set_offset(0);
    assert_eq!(cur.read_next_u8().unwrap(), b'a');
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cursor_disk_file_mixed_reads_advance_offset_correctly() {
    let mut data = vec![0xAB];
    data.extend_from_slice(&0x1234u16.to_be_bytes());
    data.extend_from_slice(&0xCAFEBABEu32.to_be_bytes());
    data.extend_from_slice(b"tail!");
    let path = temp_path("cursor_disk_mixed");
    write_temp_file(&path, &data);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::new(&file);
    assert_eq!(cur.read_next_u8().unwrap(), 0xAB);
    assert_eq!(cur.read_next_u16().unwrap(), 0x1234);
    assert_eq!(cur.read_next_u32().unwrap(), 0xCAFEBABE);
    let tail: [u8; 5] = cur.read_next_arrary::<5>().unwrap();
    assert_eq!(&tail, b"tail!");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cursor_disk_file_read_past_eof_errors() {
    let path = temp_path("cursor_disk_past_eof");
    write_temp_file(&path, &[0x01, 0x02]);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::new(&file);
    assert!(cur.read_next_u32().is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cursor_disk_file_read_array_larger_than_remaining_errors() {
    let path = temp_path("cursor_disk_array_too_big");
    write_temp_file(&path, &[1, 2, 3]);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::new(&file);
    assert!(cur.read_next_arrary::<10>().is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cursor_disk_file_two_independent_cursors() {
    let path = temp_path("cursor_disk_independent");
    write_temp_file(&path, b"0123456789");
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur_a = FileCursor::new(&file);
    let mut cur_b = FileCursor::with_offset(&file, 5);
    assert_eq!(cur_a.read_next_u8().unwrap(), b'0');
    assert_eq!(cur_b.read_next_u8().unwrap(), b'5');
    assert_eq!(cur_a.read_next_u8().unwrap(), b'1');
    assert_eq!(cur_b.read_next_u8().unwrap(), b'6');
    let _ = std::fs::remove_file(&path);
}

// ===========================================================================
// MemFile + FileCursor round-trip through header parsing
// ===========================================================================

#[test]
fn cursor_mem_file_can_read_sqlite_header_magic() {
    let header = common::valid_header_bytes(4096, 0);
    let file = MemFile::new(rc_vec(header));
    let mut cur = FileCursor::new(&file);
    let magic: [u8; 16] = cur.read_next_arrary::<16>().unwrap();
    assert_eq!(&magic, b"SQLite format 3\0");
}

#[test]
fn cursor_mem_file_can_read_page_size_from_header() {
    let header = common::valid_header_bytes(4096, 0);
    let file = MemFile::new(rc_vec(header));
    let mut cur = FileCursor::new(&file);
    let _magic: [u8; 16] = cur.read_next_arrary::<16>().unwrap();
    let page_size = cur.read_next_u16().unwrap();
    assert_eq!(page_size, 4096);
}

// ===========================================================================
// DiskFile + FileCursor round-trip through header parsing
// ===========================================================================

#[test]
fn cursor_disk_file_can_read_sqlite_header_magic() {
    let header = common::valid_header_bytes(4096, 0);
    let path = temp_path("cursor_disk_header_magic");
    write_temp_file(&path, &header);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::new(&file);
    let magic: [u8; 16] = cur.read_next_arrary::<16>().unwrap();
    assert_eq!(&magic, b"SQLite format 3\0");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cursor_disk_file_can_read_page_size_from_header() {
    let header = common::valid_header_bytes(8192, 0);
    let path = temp_path("cursor_disk_header_page_size");
    write_temp_file(&path, &header);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::new().read(true)).unwrap();
    let mut cur = FileCursor::new(&file);
    let _magic: [u8; 16] = cur.read_next_arrary::<16>().unwrap();
    let page_size = cur.read_next_u16().unwrap();
    assert_eq!(page_size, 8192);
    let _ = std::fs::remove_file(&path);
}

// ===========================================================================
// SqliteOptions flag combinations
// ===========================================================================

#[test]
fn sqlite_options_read_write_no_create() {
    let opts = SqliteOptions::new().read(true).write(true);
    assert!(opts.can_read());
    assert!(opts.can_write());
    assert!(!opts.is_create());
}

#[test]
fn sqlite_options_read_only() {
    let opts = SqliteOptions::new().read(true);
    assert!(opts.can_read());
    assert!(!opts.can_write());
    assert!(!opts.is_create());
}

#[test]
fn sqlite_options_write_only() {
    let opts = SqliteOptions::new().write(true);
    assert!(!opts.can_read());
    assert!(opts.can_write());
    assert!(!opts.is_create());
}

#[test]
fn sqlite_options_all_flags() {
    let opts = SqliteOptions::new().read(true).write(true).create(true);
    assert!(opts.can_read());
    assert!(opts.can_write());
    assert!(opts.is_create());
}

// ===========================================================================
// MemFile shared buffer mutation through multiple cursors
// ===========================================================================

#[test]
fn mem_file_write_visible_to_another_cursor() {
    let data = rc_vec(vec![0u8; 5]);
    let file_a = MemFile::new(Rc::clone(&data));
    let file_b = MemFile::new(data);

    file_a.write_all_at(0, &[1, 2, 3, 4, 5]).unwrap();

    let mut buf = [0u8; 5];
    file_b.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [1, 2, 3, 4, 5]);
}

#[test]
fn mem_file_cursor_sees_writes_from_another_handle() {
    let data = rc_vec(vec![0u8; 10]);
    let file_a = MemFile::new(Rc::clone(&data));
    let file_b = MemFile::new(data);

    file_a.write_all_at(5, &[9, 9, 9]).unwrap();

    let mut cur = FileCursor::new(&file_b);
    cur.set_offset(5);
    let mut buf = [0u8; 3];
    cur.read_next_exact(&mut buf).unwrap();
    assert_eq!(buf, [9, 9, 9]);
}
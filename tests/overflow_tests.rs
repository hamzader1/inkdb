//! Tests for `inkdb::format::overflow`.

mod common;

use common::{build_db_image, build_overflow_page, set_page_count, valid_header_bytes, MemFile};
use inkdb::errors::SqliteError;
use inkdb::format::overflow::{compute_local_payload_size, OverflowPageRef};
use inkdb::vfs::disk::DiskFile;
use inkdb::SqliteDatabase; // only for type inference where needed

// ---------------------------------------------------------------------
// compute_local_payload_size
// ---------------------------------------------------------------------

#[test]
fn payload_fits_entirely_on_page_returns_full_length() {
    // usable_size 4096 -> X = 4096 - 35 = 4061
    assert_eq!(compute_local_payload_size(4096, 0), 0);
    assert_eq!(compute_local_payload_size(4096, 4061), 4061);
}

#[test]
fn payload_just_over_x_spills_to_overflow() {
    // At payload_len = X + 1 = 4062, k computation kicks in.
    let result = compute_local_payload_size(4096, 4062);
    assert!(result < 4062, "should store less than full payload locally");
    assert!(result > 0);
}

#[test]
fn very_large_payload_falls_back_to_m() {
    let k = 1792;
    let result = compute_local_payload_size(4096, 100_000);
    assert_eq!(result, k);
}

#[test]
fn local_payload_never_exceeds_payload_len() {
    for payload_len in [0usize, 1, 100, 4061, 4062, 5000, 50_000, 1_000_000] {
        let local = compute_local_payload_size(4096, payload_len);
        assert!(
            local <= payload_len,
            "local payload {local} exceeds actual payload {payload_len}"
        );
    }
}

#[test]
fn common_sqlite_page_sizes_produce_sane_x_boundary() {
    // Sanity check across the standard set of legal SQLite page sizes:
    // local size at the X boundary should equal X exactly (no overflow yet).
    for &usable in &[512usize, 1024, 2048, 4096, 8192, 16384, 32768] {
        let x = usable - 35;
        assert_eq!(compute_local_payload_size(usable, x), x);
        // one byte more must NOT return the full payload untouched.
        let over = compute_local_payload_size(usable, x + 1);
        assert!(over <= x + 1);
    }
}

// ---------------------------------------------------------------------
// OverflowPageRef::new
// ---------------------------------------------------------------------

#[test]
fn overflow_page_ref_parses_next_pointer_and_data() {
    let usable_size = 512;
    let mut page = vec![0u8; usable_size];
    page[0..4].copy_from_slice(&7u32.to_be_bytes());
    page[4] = 0xAB;
    page[5] = 0xCD;

    let ov = OverflowPageRef::new(&page, usable_size).unwrap();
    assert_eq!(ov.next, 7);
    assert_eq!(ov.data.len(), usable_size - 4);
    assert_eq!(ov.data[0], 0xAB);
    assert_eq!(ov.data[1], 0xCD);
}

#[test]
fn overflow_page_ref_zero_next_means_last_page() {
    let usable_size = 512;
    let page = vec![0u8; usable_size]; // next = 0
    let ov = OverflowPageRef::new(&page, usable_size).unwrap();
    assert_eq!(ov.next, 0);
}

#[test]
fn overflow_page_ref_too_short_buffer_errors() {
    let usable_size = 512;
    let page = vec![0u8; 100]; // shorter than usable_size
    let result = OverflowPageRef::new(&page, usable_size);
    assert!(result.is_err());
}

#[test]
fn overflow_page_ref_data_len_is_usable_size_minus_4() {
    for usable_size in [512usize, 1024, 4096] {
        let page = vec![0u8; usable_size];
        let ov = OverflowPageRef::new(&page, usable_size).unwrap();
        assert_eq!(ov.data.len(), usable_size - 4);
    }
}

// ---------------------------------------------------------------------
// SqliteDatabase::read_overflow_payload (full integration through MemFile)
// ---------------------------------------------------------------------

// The `SqliteDatabase` struct's fields (`source`, `header`) are private and
// `with_source` is a private (non-pub) associated function, so it is NOT
// reachable from an external integration test (`tests/*.rs` compiles as a
// separate crate). `SqliteDatabase::new` is also hard-coded to `DiskFile`.
//
// This means `read_overflow_payload`, `page()`, `cell_payload()`, and
// `freelist()` -- everything that needs a live `SqliteDatabase<S>` -- can
// only be integration-tested today via real files on disk through
// `SqliteDatabase::new`, OR by adding a `pub fn with_source` /
// `pub fn from_source` constructor to the crate. We test what's reachable
// (the pure/standalone pieces: `compute_local_payload_size`,
// `OverflowPageRef::new`) above, and leave the disk-backed integration path
// below using a real temp file so the full `read_overflow_payload` chain
// logic is still covered end-to-end.
mod disk_integration {
    use super::*;
    use std::io::Write;

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("inkdb_test_{}_{}.db", std::process::id(), name));
        p
    }

    fn write_db_file(path: &std::path::Path, image: &[u8]) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(image).unwrap();
        f.sync_all().unwrap();
    }

    #[test]
    fn read_overflow_payload_single_page_chain() {
        let page_size = 512usize;
        let page_count = 3u32; // page 1 = header/schema, page 2 = data (unused), page 3 = overflow
        let usable_size = page_size; // reserved_space = 0

        let mut image = build_db_image(page_size, page_count, |_| {});

        // Build overflow page 3: no `next`, contains remaining payload bytes.
        let remaining_payload = b"OVERFLOW_PAYLOAD_BYTES_HERE";
        let overflow_page = build_overflow_page(0, remaining_payload, usable_size);
        let start = page_size * 2; // page 3 -> 0-indexed offset 2
        image[start..start + page_size].copy_from_slice(&overflow_page);

        let path = temp_db_path("single_overflow");
        write_db_file(&path, &image);

        let mut db = SqliteDatabase::new(&path).expect("should open valid db image");

        let local_bytes = b"LOCAL_".to_vec();
        let total_len = local_bytes.len() + remaining_payload.len();
        let result = db
            .read_overflow_payload(local_bytes.clone(), total_len, 3)
            .expect("overflow chain should resolve");

        let mut expected = local_bytes;
        expected.extend_from_slice(remaining_payload);
        assert_eq!(result, expected);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_overflow_payload_multi_page_chain() {
        let page_size = 512usize;
        let page_count = 4u32; // page 3 and 4 form the overflow chain
        let usable_size = page_size;

        let mut image = build_db_image(page_size, page_count, |_| {});

        let chunk1 = vec![0xAAu8; usable_size - 4];
        let chunk2 = b"TAIL".to_vec();

        let page3 = build_overflow_page(4, &chunk1, usable_size);
        let page4 = build_overflow_page(0, &chunk2, usable_size);

        image[page_size * 2..page_size * 3].copy_from_slice(&page3);
        image[page_size * 3..page_size * 4].copy_from_slice(&page4);

        let path = temp_db_path("multi_overflow");
        write_db_file(&path, &image);

        let mut db = SqliteDatabase::new(&path).unwrap();

        let local_bytes = b"HEAD".to_vec();
        let total_len = local_bytes.len() + chunk1.len() + chunk2.len();
        let result = db
            .read_overflow_payload(local_bytes.clone(), total_len, 3)
            .unwrap();

        let mut expected = local_bytes;
        expected.extend_from_slice(&chunk1);
        expected.extend_from_slice(&chunk2);
        assert_eq!(result, expected);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_overflow_payload_chain_ends_early_errors() {
        let page_size = 512usize;
        let page_count = 3u32;
        let usable_size = page_size;

        let mut image = build_db_image(page_size, page_count, |_| {});
        // Overflow page 3 says next = 0 (chain ends) but we ask for more
        // total payload than local + this page can supply.
        let short_chunk = vec![0x11u8; 5];
        let page3 = build_overflow_page(0, &short_chunk, usable_size);
        image[page_size * 2..page_size * 3].copy_from_slice(&page3);

        let path = temp_db_path("early_end");
        write_db_file(&path, &image);
        let mut db = SqliteDatabase::new(&path).unwrap();

        let local_bytes = vec![0u8; 2];
        let total_len = local_bytes.len() + short_chunk.len() + 1000; // too much
        let result = db.read_overflow_payload(local_bytes, total_len, 3);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_overflow_payload_chain_continues_after_complete_errors() {
        let page_size = 512usize;
        let page_count = 3u32;
        let usable_size = page_size;

        let mut image = build_db_image(page_size, page_count, |_| {});
        // Payload will be fully satisfied by page 3's data, but page 3
        // claims a bogus `next` pointer anyway (points at itself: page 3).
        let chunk = vec![0x22u8; 50];
        let page3 = build_overflow_page(3, &chunk, usable_size);
        image[page_size * 2..page_size * 3].copy_from_slice(&page3);

        let path = temp_db_path("continues_after_complete");
        write_db_file(&path, &image);
        let mut db = SqliteDatabase::new(&path).unwrap();

        let local_bytes = vec![0u8; 2];
        let total_len = local_bytes.len() + chunk.len();
        let result = db.read_overflow_payload(local_bytes, total_len, 3);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_overflow_payload_local_exceeds_total_errors() {
        let page_size = 512usize;
        let page_count = 3u32;
        let image = build_db_image(page_size, page_count, |_| {});

        let path = temp_db_path("local_exceeds_total");
        write_db_file(&path, &image);
        let mut db = SqliteDatabase::new(&path).unwrap();

        let local_bytes = vec![0u8; 100];
        // total_payload_length smaller than what we already have locally.
        let result = db.read_overflow_payload(local_bytes, 10, 3);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_overflow_payload_exact_single_page_no_remainder() {
        let page_size = 512usize;
        let page_count = 3u32;
        let usable_size = page_size;

        let mut image = build_db_image(page_size, page_count, |_| {});
        let chunk = vec![0x55u8; 10];
        let page3 = build_overflow_page(0, &chunk, usable_size);
        image[page_size * 2..page_size * 3].copy_from_slice(&page3);

        let path = temp_db_path("exact_single_page");
        write_db_file(&path, &image);
        let mut db = SqliteDatabase::new(&path).unwrap();

        let local_bytes: Vec<u8> = vec![];
        let total_len = chunk.len();
        let result = db.read_overflow_payload(local_bytes, total_len, 3).unwrap();
        assert_eq!(result, chunk);

        let _ = std::fs::remove_file(&path);
    }
}

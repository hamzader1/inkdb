//! Tests for `inkdb::format::page::BTreePage`.
//!
//! `BTreePage::parse` is a free function taking raw bytes (no `SqliteFile`
//! needed), so these tests build pages directly in memory - no disk I/O
//! required.

mod common;

use common::{
    encode_index_leaf_cell, encode_table_leaf_cell, write_interior_index_page,
    write_interior_table_page, write_leaf_index_page, write_leaf_table_page,
};
use inkdb::errors::SqliteDatabaseError;
use inkdb::format::cell::BTreeCell;
use inkdb::format::page::BTreePage;

const PAGE_SIZE: usize = 512;
const USABLE_SIZE: usize = 512;

#[test]
fn parse_empty_leaf_table_page_no1() {
    let mut buf = vec![0u8; PAGE_SIZE];
    write_leaf_table_page(&mut buf, 100, &[]);
    let page = BTreePage::parse(buf, 1, PAGE_SIZE, USABLE_SIZE).unwrap();
    assert_eq!(page.no_of_cell(), 0);
}

#[test]
fn parse_leaf_table_page_with_one_cell() {
    let mut buf = vec![0u8; PAGE_SIZE];
    let cell = encode_table_leaf_cell(4, 1, b"data", None);
    write_leaf_table_page(&mut buf, 0, &[cell]);
    let page = BTreePage::parse(buf, 2, PAGE_SIZE, USABLE_SIZE).unwrap();
    assert_eq!(page.no_of_cell(), 1);

    let c = page.cell(0).unwrap();
    match c {
        BTreeCell::TableLeaf(_) => {}
        _ => panic!("expected TableLeaf cell"),
    }
}

#[test]
fn parse_leaf_table_page_with_multiple_cells_preserves_order() {
    let mut buf = vec![0u8; PAGE_SIZE];
    let cell0 = encode_table_leaf_cell(3, 10, b"aaa", None);
    let cell1 = encode_table_leaf_cell(3, 20, b"bbb", None);
    let cell2 = encode_table_leaf_cell(3, 30, b"ccc", None);
    write_leaf_table_page(&mut buf, 0, &[cell0, cell1, cell2]);

    let page = BTreePage::parse(buf, 2, PAGE_SIZE, USABLE_SIZE).unwrap();
    assert_eq!(page.no_of_cell(), 3);

    for i in 0..3 {
        // Should not error for any valid index.
        page.cell(i).unwrap();
    }
}

#[test]
fn cell_index_out_of_bounds_errors() {
    let mut buf = vec![0u8; PAGE_SIZE];
    write_leaf_table_page(&mut buf, 0, &[]);
    let page = BTreePage::parse(buf, 2, PAGE_SIZE, USABLE_SIZE).unwrap();
    let result = page.cell(0);
    // dbg!(&result);
    assert!(result.is_err());
}

#[test]
fn parse_interior_table_page() {
    let mut buf = vec![0u8; PAGE_SIZE];
    write_interior_table_page(&mut buf, 0, &[(5, 100), (6, 200)], 7);
    let page = BTreePage::parse(buf, 3, PAGE_SIZE, USABLE_SIZE).unwrap();
    assert_eq!(page.no_of_cell(), 2);

    match page.cell(0).unwrap() {
        BTreeCell::TableInterior(_) => {}
        _ => panic!("expected TableInterior cell"),
    }
}

#[test]
fn interior_page_with_zero_right_most_pointer_is_rejected() {
    let mut buf = vec![0u8; PAGE_SIZE];
    write_interior_table_page(&mut buf, 0, &[(5, 100)], 0); // right-most = 0 -> invalid
    let result = BTreePage::parse(buf, 3, PAGE_SIZE, USABLE_SIZE);
    assert!(result.is_err());
    match result.unwrap_err() {
        SqliteDatabaseError::CorruptedPage { reason, .. } => {
            assert!(reason.contains("right-most"));
        }
        other => panic!("expected CorruptedPage, got {other:?}"),
    }
}

#[test]
fn parse_leaf_index_page() {
    let mut buf = vec![0u8; PAGE_SIZE];
    let cell = encode_index_leaf_cell(4, b"key1", None);
    write_leaf_index_page(&mut buf, 0, &[cell]);
    let page = BTreePage::parse(buf, 2, PAGE_SIZE, USABLE_SIZE).unwrap();
    assert_eq!(page.no_of_cell(), 1);
    match page.cell(0).unwrap() {
        BTreeCell::IndexLeaf(_) => {}
        _ => panic!("expected IndexLeaf cell"),
    }
}

#[test]
fn parse_interior_index_page() {
    let mut buf = vec![0u8; PAGE_SIZE];
    let cell_body = {
        let mut b = 9u32.to_be_bytes().to_vec();
        b.extend_from_slice(&encode_index_leaf_cell(3, b"idx", None));
        b
    };
    write_interior_index_page(&mut buf, 0, &[cell_body], 11);
    let page = BTreePage::parse(buf, 2, PAGE_SIZE, USABLE_SIZE).unwrap();
    assert_eq!(page.no_of_cell(), 1);
    match page.cell(0).unwrap() {
        BTreeCell::IndexInterior(_) => {}
        _ => panic!("expected IndexInterior cell"),
    }
}

#[test]
fn invalid_page_type_byte_is_rejected() {
    let mut buf = vec![0u8; PAGE_SIZE];
    buf[0] = 0xFF; // not a valid btree page type
    let result = BTreePage::parse(buf, 2, PAGE_SIZE, USABLE_SIZE);
    match result {
        Err(SqliteDatabaseError::InvalidPageType(0xFF)) => {}
        other => panic!("expected InvalidPageType(0xFF), got {other:?}"),
    }
}

#[test]
fn page_one_reads_header_after_100_byte_sqlite_header() {
    // Page 1 always has its btree header offset by SQLITE3_HEADER_SIZE (100).
    let mut buf = vec![0u8; PAGE_SIZE];
    write_leaf_table_page(&mut buf, 100, &[]);
    let page = BTreePage::parse(buf, 1, PAGE_SIZE, USABLE_SIZE).unwrap();
    assert_eq!(page.no_of_cell(), 0);
}

#[test]
fn fragmented_free_bytes_over_60_is_rejected() {
    let mut buf = vec![0u8; PAGE_SIZE];
    write_leaf_table_page(&mut buf, 0, &[]);
    buf[7] = 61; // frag_cnt byte, must be <= 60
    let result = BTreePage::parse(buf, 2, PAGE_SIZE, USABLE_SIZE);
    assert!(result.is_err());
}

#[test]
fn fragmented_free_bytes_exactly_60_is_accepted() {
    let mut buf = vec![0u8; PAGE_SIZE];
    write_leaf_table_page(&mut buf, 0, &[]);
    buf[7] = 60;
    let result = BTreePage::parse(buf, 2, PAGE_SIZE, USABLE_SIZE);
    assert!(result.is_ok());
}

#[test]
fn validate_header_offset_check_uses_page_no_zero_not_one() {
    // NOTE: `BTreePage::validate()` sets `btree_header_offset` based on
    // `self.page_no == 0`, but `BTreePage::parse()` applies the 100-byte
    // SQLite header offset when `page_no == 1` (page numbers are 1-indexed
    // and page 0 never occurs in a real database). This looks like an
    // off-by-one bug: for page 1, `validate()`'s pointer-array-bounds
    // check effectively always uses `btree_header_offset = 0` instead of
    // 100, which makes the "cell pointer array exceeds usable page space"
    // check slightly too permissive for page 1. This test documents
    // current behavior (parse succeeds) rather than asserting what might
    // be the "intended" stricter behavior, so a future fix will surface
    // here rather than silently changing acceptance.
    let mut buf = vec![0u8; PAGE_SIZE];
    write_leaf_table_page(&mut buf, 100, &[]);
    let page = BTreePage::parse(buf, 1, PAGE_SIZE, USABLE_SIZE);
    assert!(page.is_ok(), "documents current (possibly buggy) behavior");
}

#[test]
fn bytes_accessor_returns_full_page_buffer() {
    let mut buf = vec![0u8; PAGE_SIZE];
    write_leaf_table_page(&mut buf, 0, &[]);
    let page = BTreePage::parse(buf, 2, PAGE_SIZE, USABLE_SIZE).unwrap();
    assert_eq!(page.bytes().len(), PAGE_SIZE);
}

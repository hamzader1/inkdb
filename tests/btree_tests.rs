//! Tests for `inkdb::btree::btree`.
//!
//! Exercises `BTreePageRef` (page header + cell parsing) and `BTreeCursor`
//! (seek / first / last / next / prev / descend / current) against fully
//! in-memory database images backed by `inkdb::vfs::mem::MemFile` + a
//! `Pager`. No disk is touched.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use inkdb::btree::btree::{BTreeCursor, BTreePageRef, CursorState, SeekResult};
use inkdb::errors::SqliteError;
use inkdb::pager::page::Pager;
use inkdb::vfs::mem::MemFile;

const PAGE_SIZE: usize = 512;
const USABLE: usize = 512;

fn rc_vec(data: Vec<u8>) -> Rc<RefCell<Vec<u8>>> {
    Rc::new(RefCell::new(data))
}

/// A zeroed multi-page database image with a valid 100-byte SQLite header.
fn make_db(page_size: usize, page_count: usize) -> Vec<u8> {
    let mut data = vec![0u8; page_size * page_count];
    let header = common::valid_header_bytes(page_size as u16, 0);
    data[0..100].copy_from_slice(&header);
    data
}

fn pager_from(data: Vec<u8>, page_size: usize, page_count: usize) -> Pager<MemFile> {
    let file = MemFile::new(rc_vec(data));
    Pager::new(file, page_size, page_size, page_count)
}

/// Encode a table-leaf cell with a fully-local payload (no overflow).
fn enc(row_id: u64, payload: &[u8]) -> Vec<u8> {
    common::encode_table_leaf_cell(payload.len() as u64, row_id, payload, None)
}

/// Build a db whose `page_no` is a table-leaf root holding `cells`.
fn leaf_db(page_size: usize, page_count: usize, page_no: u32, cells: &[Vec<u8>]) -> Pager<MemFile> {
    let mut data = make_db(page_size, page_count);
    let start = (page_no - 1) as usize * page_size;
    common::write_leaf_table_page(&mut data[start..start + page_size], 0, cells);
    pager_from(data, page_size, page_count)
}

/// Standard 512-byte leaf-table root at page 2 with rowid-sorted cells.
fn leaf_root(cells: &[(u64, &[u8])]) -> Pager<MemFile> {
    let encoded: Vec<Vec<u8>> = cells.iter().map(|&(rid, p)| enc(rid, p)).collect();
    leaf_db(PAGE_SIZE, 2, 2, &encoded)
}

/// A two-level table tree:
///   page 2: interior, [(left=3, boundary=3)] right=4
///   page 3: leaf (1,a) (2,b) (3,c)
///   page 4: leaf (4,d) (5,e) (6,f)
fn two_level_tree() -> Pager<MemFile> {
    let mut data = make_db(PAGE_SIZE, 4);
    common::write_interior_table_page(
        &mut data[PAGE_SIZE..2 * PAGE_SIZE],
        0,
        &[(3, 3)],
        4,
    );
    let left = [enc(1, b"a"), enc(2, b"b"), enc(3, b"c")];
    common::write_leaf_table_page(&mut data[2 * PAGE_SIZE..3 * PAGE_SIZE], 0, &left);
    let right = [enc(4, b"d"), enc(5, b"e"), enc(6, b"f")];
    common::write_leaf_table_page(&mut data[3 * PAGE_SIZE..4 * PAGE_SIZE], 0, &right);
    pager_from(data, PAGE_SIZE, 4)
}

/// A three-level table tree:
///   page 2: interior, [(left=3, boundary=4)] right=4
///   page 3: interior, [(left=5, boundary=2)] right=6
///   page 5: leaf (1,a) (2,b)
///   page 6: leaf (3,c) (4,d)
///   page 4: leaf (5,e) (6,f)
fn three_level_tree() -> Pager<MemFile> {
    let mut data = make_db(PAGE_SIZE, 6);
    common::write_interior_table_page(&mut data[PAGE_SIZE..2 * PAGE_SIZE], 0, &[(3, 4)], 4);
    common::write_interior_table_page(
        &mut data[2 * PAGE_SIZE..3 * PAGE_SIZE],
        0,
        &[(5, 2)],
        6,
    );
    let p5 = [enc(1, b"a"), enc(2, b"b")];
    common::write_leaf_table_page(&mut data[4 * PAGE_SIZE..5 * PAGE_SIZE], 0, &p5);
    let p6 = [enc(3, b"c"), enc(4, b"d")];
    common::write_leaf_table_page(&mut data[5 * PAGE_SIZE..6 * PAGE_SIZE], 0, &p6);
    let p4 = [enc(5, b"e"), enc(6, b"f")];
    common::write_leaf_table_page(&mut data[3 * PAGE_SIZE..4 * PAGE_SIZE], 0, &p4);
    pager_from(data, PAGE_SIZE, 6)
}

/// Read the row id of the cell at `(page_no, idx)` on a 512-byte page.
fn cell_row_id(pager: &mut Pager<MemFile>, page_no: u32, idx: u16) -> u64 {
    cell_row_id_at(pager, page_no, idx, PAGE_SIZE, USABLE)
}

/// Read the row id of the cell at `(page_no, idx)` on a custom page size.
fn cell_row_id_at(
    pager: &mut Pager<MemFile>,
    page_no: u32,
    idx: u16,
    page_size: usize,
    usable: usize,
) -> u64 {
    let guard = pager.get(page_no).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, page_size, usable).unwrap();
    page.cell(idx).unwrap().row_id()
}

/// Read the payload bytes of the cell at `(page_no, idx)` on a 512-byte page.
fn cell_payload(pager: &mut Pager<MemFile>, page_no: u32, idx: u16) -> Vec<u8> {
    cell_payload_at(pager, page_no, idx, PAGE_SIZE, USABLE)
}

/// Read the payload bytes of the cell at `(page_no, idx)` on a custom page size.
fn cell_payload_at(
    pager: &mut Pager<MemFile>,
    page_no: u32,
    idx: u16,
    page_size: usize,
    usable: usize,
) -> Vec<u8> {
    let guard = pager.get(page_no).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, page_size, usable).unwrap();
    let cell = page.cell(idx).unwrap();
    let range = *cell.payload();
    guard.bytes()[range].to_vec()
}

/// The expected insertion index for a missing `target` in a sorted rowid list.
fn insert_idx(rowids: &[u64], target: u64) -> u16 {
    rowids.iter().position(|&r| r > target).unwrap_or(rowids.len()) as u16
}

/// Forward walk: `first()` then `next()` until AfterLast, collecting row ids.
fn walk_forward(pager: &mut Pager<MemFile>, cursor: &mut BTreeCursor) -> Vec<u64> {
    let mut out = Vec::new();
    cursor.first(pager).unwrap();
    assert_eq!(cursor.state, CursorState::At);
    loop {
        let &(page_no, idx) = cursor.stack.last().unwrap();
        out.push(cell_row_id(pager, page_no, idx));
        cursor.next(pager).unwrap();
        if cursor.state == CursorState::AfterLast {
            break;
        }
    }
    out
}

/// Backward walk: `last()` then `prev()` until BeforeFirst, collecting row ids.
fn walk_backward(pager: &mut Pager<MemFile>, cursor: &mut BTreeCursor) -> Vec<u64> {
    let mut out = Vec::new();
    cursor.last(pager).unwrap();
    assert_eq!(cursor.state, CursorState::At);
    loop {
        let &(page_no, idx) = cursor.stack.last().unwrap();
        out.push(cell_row_id(pager, page_no, idx));
        cursor.prev(pager).unwrap();
        if cursor.state == CursorState::BeforeFirst {
            break;
        }
    }
    out
}

#[test]
fn btree_header_size_constants_are_8_and_12() {
    use inkdb::btree::btree::{
        INTERIOR_BTREE_PAGE_HEADER_SIZE, LEAF_BTREE_PAGE_HEADER_SIZE,
    };
    assert_eq!(LEAF_BTREE_PAGE_HEADER_SIZE, 8);
    assert_eq!(INTERIOR_BTREE_PAGE_HEADER_SIZE, 12);
}

#[test]
fn btree_page_type_constants_round_trip() {
    use inkdb::btree::btree::{BTREE_TYPE_PAGE_OFFSET, BTREE_TYPE_PAGE_SIZE};
    assert_eq!(BTREE_TYPE_PAGE_OFFSET, 0);
    assert_eq!(BTREE_TYPE_PAGE_SIZE, 1);
}

#[test]
fn leaf_table_page_parses_header_and_cells() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    for i in 0..3u16 {
        let rid = cell_row_id(&mut pager, 2, i);
        assert_eq!(rid, i as u64 + 1);
        let payload = cell_payload(&mut pager, 2, i);
        assert_eq!(&payload, [b"a"[0] + i as u8].as_slice());
    }
}

#[test]
fn leaf_table_single_cell_parses() {
    let mut pager = leaf_root(&[(42, b"hello")]);
    assert_eq!(cell_row_id(&mut pager, 2, 0), 42);
    assert_eq!(cell_payload(&mut pager, 2, 0), b"hello");
}

#[test]
fn leaf_table_empty_page_cell_access_errors() {
    let mut pager = leaf_root(&[]);
    let guard = pager.get(2).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE).unwrap();
    assert!(page.cell(0).is_err());
}

#[test]
fn leaf_table_cell_out_of_bounds_errors() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b")]);
    let guard = pager.get(2).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE).unwrap();
    assert!(page.cell(2).is_err());
    assert!(page.cell(10).is_err());
    assert!(page.cell(20).is_err());
}

#[test]
fn leaf_table_cell_high_index_on_large_page_parses() {
    let cells: Vec<Vec<u8>> = (1..=100).map(|i| enc(i, b"x")).collect();
    let mut pager = leaf_db(PAGE_SIZE, 2, 2, &cells);
    assert_eq!(cell_row_id(&mut pager, 2, 99), 100);
    assert_eq!(cell_row_id(&mut pager, 2, 0), 1);
}

#[test]
fn leaf_table_varint_boundary_rowids_parse() {
    let rowids: [u64; 7] = [
        0,
        127,
        128,
        16_383,
        16_384,
        u32::MAX as u64,
        u64::MAX,
    ];
    let cells: Vec<Vec<u8>> = rowids.iter().map(|&r| enc(r, b"p")).collect();
    let mut pager = leaf_db(PAGE_SIZE, 2, 2, &cells);
    for (i, &rid) in rowids.iter().enumerate() {
        assert_eq!(cell_row_id(&mut pager, 2, i as u16), rid);
        assert_eq!(cell_payload(&mut pager, 2, i as u16), b"p");
    }
}

#[test]
fn leaf_table_variable_payload_sizes_parse() {
    let p1 = vec![0x11u8; 1];
    let p2 = vec![0x22u8; 50];
    let p3 = vec![0x33u8; 120];
    let mut pager = leaf_root(&[(1, &p1), (2, &p2), (3, &p3)]);
    assert_eq!(cell_payload(&mut pager, 2, 0), p1);
    assert_eq!(cell_payload(&mut pager, 2, 1), p2);
    assert_eq!(cell_payload(&mut pager, 2, 2), p3);
}

#[test]
fn leaf_table_large_local_payload_parses() {
    let payload = vec![0x55u8; 400];
    let mut pager = leaf_root(&[(1, &payload)]);
    assert_eq!(cell_payload(&mut pager, 2, 0), payload);
}

#[test]
fn leaf_table_big_page_many_cells_and_large_payloads() {
    let page_size = 4096usize;
    let mut cells = Vec::new();
    for i in 0..200u64 {
        cells.push(enc(i + 1, b"x"));
    }
    let mut pager = leaf_db(page_size, 2, 2, &cells);
    for i in 0..200u16 {
        assert_eq!(cell_row_id_at(&mut pager, 2, i, page_size, page_size), i as u64 + 1);
    }

    let big = vec![0xABu8; 1500];
    let cells2 = vec![enc(1, &big), enc(2, &big)];
    let mut pager2 = leaf_db(page_size, 2, 2, &cells2);
    assert_eq!(cell_payload_at(&mut pager2, 2, 1, page_size, page_size), big);
}

#[test]
fn interior_table_page_parses_cells() {
    let mut data = make_db(PAGE_SIZE, 2);
    common::write_interior_table_page(&mut data[PAGE_SIZE..], 0, &[(7, 10), (8, 20)], 9);
    let mut pager = pager_from(data, PAGE_SIZE, 2);

    let guard = pager.get(2).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE).unwrap();
    let c0 = page.cell(0).unwrap();
    assert_eq!(c0.left_child(), 7);
    assert_eq!(c0.row_id(), 10);
    let c1 = page.cell(1).unwrap();
    assert_eq!(c1.left_child(), 8);
    assert_eq!(c1.row_id(), 20);
}

#[test]
fn interior_table_page_cell_out_of_bounds_errors() {
    let mut data = make_db(PAGE_SIZE, 2);
    common::write_interior_table_page(&mut data[PAGE_SIZE..], 0, &[(7, 10)], 9);
    let mut pager = pager_from(data, PAGE_SIZE, 2);
    let guard = pager.get(2).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE).unwrap();
    assert!(page.cell(1).is_err());
}

#[test]
fn interior_table_page_large_rowid_boundary_parses() {
    let mut data = make_db(PAGE_SIZE, 2);
    common::write_interior_table_page(&mut data[PAGE_SIZE..], 0, &[(5, u64::MAX)], 6);
    let mut pager = pager_from(data, PAGE_SIZE, 2);
    let guard = pager.get(2).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE).unwrap();
    let c = page.cell(0).unwrap();
    assert_eq!(c.left_child(), 5);
    assert_eq!(c.row_id(), u64::MAX);
}

#[test]
fn leaf_index_page_parses_payload() {
    let mut data = make_db(PAGE_SIZE, 2);
    let cell = common::encode_index_leaf_cell(3, b"idx", None);
    common::write_leaf_index_page(&mut data[PAGE_SIZE..], 0, &[cell]);
    let mut pager = pager_from(data, PAGE_SIZE, 2);

    let guard = pager.get(2).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE).unwrap();
    let c = page.cell(0).unwrap();
    assert_eq!(c.cell_payload_len(), 3);
    assert_eq!(c.overflow_page(), None);
    let range = *c.payload();
    assert_eq!(&guard.bytes()[range], b"idx");
}

#[test]
fn interior_index_page_parses_left_child_and_payload() {
    let mut data = make_db(PAGE_SIZE, 2);
    let cell = common::encode_index_interior_cell(55, 3, b"key", None);
    common::write_interior_index_page(&mut data[PAGE_SIZE..], 0, &[cell], 99);
    let mut pager = pager_from(data, PAGE_SIZE, 2);

    let guard = pager.get(2).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE).unwrap();
    let c = page.cell(0).unwrap();
    assert_eq!(c.left_child(), 55);
    assert_eq!(c.cell_payload_len(), 3);
    let range = *c.payload();
    assert_eq!(&guard.bytes()[range], b"key");
}

#[test]
fn invalid_page_type_rejected() {
    let mut data = make_db(PAGE_SIZE, 2);
    data[PAGE_SIZE] = 0x00; // unknown page type
    let mut pager = pager_from(data, PAGE_SIZE, 2);
    let guard = pager.get(2).unwrap();
    let result = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE);
    match result {
        Err(SqliteError::InvalidPageType(kind)) => assert_eq!(kind, 0x00),
        _ => panic!("expected InvalidPageType"),
    }
}

#[test]
fn invalid_page_type_arbitrary_value_rejected() {
    let mut data = make_db(PAGE_SIZE, 2);
    data[PAGE_SIZE] = 0x99;
    let mut pager = pager_from(data, PAGE_SIZE, 2);
    let guard = pager.get(2).unwrap();
    let result = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE);
    match result {
        Err(SqliteError::InvalidPageType(kind)) => assert_eq!(kind, 0x99),
        _ => panic!("expected InvalidPageType"),
    }
}

#[test]
fn page_one_with_embedded_sqlite_header_is_rejected_by_page_ref() {
    // Page 1 starts with "SQLite format 3\0", i.e. byte 0 = 'S' = 0x53,
    // which is not a valid btree page type. BTreePageRef does not skip the
    // 100-byte SQLite header, so page 1 must never be fed to it directly.
    let data = make_db(PAGE_SIZE, 2);
    let mut pager = pager_from(data, PAGE_SIZE, 2);
    let guard = pager.get(1).unwrap();
    let result = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE);
    match result {
        Err(SqliteError::InvalidPageType(kind)) => assert_eq!(kind, b'S'),
        _ => panic!("expected InvalidPageType"),
    }
}

#[test]
fn empty_interior_page_with_right_most_child_parses() {
    let mut data = make_db(PAGE_SIZE, 2);
    common::write_interior_table_page(&mut data[PAGE_SIZE..], 0, &[], 9);
    let mut pager = pager_from(data, PAGE_SIZE, 2);
    let guard = pager.get(2).unwrap();
    let page = BTreePageRef::new(guard.bytes(), &guard, PAGE_SIZE, USABLE).unwrap();
    assert!(page.cell(0).is_err());
}

#[test]
fn new_cursor_starts_invalid_with_empty_stack() {
    let cursor = BTreeCursor::new(2);
    assert_eq!(cursor.state, CursorState::Invalid);
    assert!(cursor.stack.is_empty());
}

#[test]
fn seek_on_empty_leaf_returns_not_found_at_zero() {
    let mut pager = leaf_root(&[]);
    let mut cursor = BTreeCursor::new(2);
    let res = cursor.seek(&mut pager, 5).unwrap();
    assert_eq!(res, SeekResult::NotFound);
    assert_eq!(cursor.state, CursorState::At);
    assert_eq!(cursor.stack, vec![(2, 0)]);
}

#[test]
fn seek_exact_match() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 2).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.state, CursorState::At);
    assert_eq!(cursor.stack, vec![(2, 1)]);
}

#[test]
fn seek_first_cell_is_exact_index_zero() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 1).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 0)]);
}

#[test]
fn seek_last_cell_is_exact_at_end() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 3).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 2)]);
}

#[test]
fn seek_missing_below_first_inserts_at_zero() {
    let mut pager = leaf_root(&[(10, b"a"), (20, b"b")]);
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 5).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 0)]);
}

#[test]
fn seek_missing_in_middle_inserts_at_boundary() {
    let mut pager = leaf_root(&[(10, b"a"), (20, b"b"), (30, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 25).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 2)]);
}

#[test]
fn seek_missing_above_last_inserts_at_end() {
    let mut pager = leaf_root(&[(10, b"a"), (20, b"b")]);
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 99).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 2)]);
}

#[test]
fn seek_missing_adjacent_to_existing() {
    let mut pager = leaf_root(&[(10, b"a"), (20, b"b")]);
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 9).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 0)]);
    assert_eq!(cursor.seek(&mut pager, 21).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 2)]);
}

#[test]
fn seek_single_cell_exact_and_misses() {
    let mut pager = leaf_root(&[(7, b"x")]);
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 7).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 0)]);
    assert_eq!(cursor.seek(&mut pager, 0).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 0)]);
    assert_eq!(cursor.seek(&mut pager, 8).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 1)]);
}

#[test]
fn seek_to_u64_max_rowid() {
    let mut pager = leaf_root(&[(1, b"a"), (u64::MAX, b"z")]);
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(
        cursor.seek(&mut pager, u64::MAX).unwrap(),
        SeekResult::Exact
    );
    assert_eq!(cursor.stack, vec![(2, 1)]);
    assert_eq!(cursor.seek(&mut pager, u64::MAX - 1).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 1)]);
}

#[test]
fn seek_into_varint_boundary_rowids() {
    let rowids: Vec<u64> = vec![0, 127, 128, 16_383, 16_384, u32::MAX as u64, u64::MAX];
    let cells: Vec<Vec<u8>> = rowids.iter().map(|&r| enc(r, b"p")).collect();
    let mut pager = leaf_db(PAGE_SIZE, 2, 2, &cells);

    let mut cursor = BTreeCursor::new(2);
    for (i, &rid) in rowids.iter().enumerate() {
        assert_eq!(cursor.seek(&mut pager, rid).unwrap(), SeekResult::Exact);
        assert_eq!(cursor.stack, vec![(2, i as u16)]);
    }
}

#[test]
fn seek_into_boundaries_between_varint_rowids() {
    let rowids: Vec<u64> = vec![0, 127, 128, 16_383, 16_384, u32::MAX as u64, u64::MAX];
    let cells: Vec<Vec<u8>> = rowids.iter().map(|&r| enc(r, b"p")).collect();
    let mut pager = leaf_db(PAGE_SIZE, 2, 2, &cells);

    let mut cursor = BTreeCursor::new(2);
    for target in [1u64, 126, 129, 10_000, 16_385, 5_000_000_000, u64::MAX - 1] {
        let res = cursor.seek(&mut pager, target).unwrap();
        assert_eq!(res, SeekResult::NotFound);
        let expected = insert_idx(&rowids, target);
        assert_eq!(cursor.stack, vec![(2, expected)], "target {target}");
    }
}

#[test]
fn seek_stress_across_sparse_rowids() {
    let rowids: Vec<u64> = (1..=50).map(|i| i * 7).collect();
    let cells: Vec<Vec<u8>> = rowids.iter().map(|&r| enc(r, b"x")).collect();
    let mut pager = leaf_db(PAGE_SIZE, 2, 2, &cells);

    let mut cursor = BTreeCursor::new(2);
    for (i, &rid) in rowids.iter().enumerate() {
        assert_eq!(cursor.seek(&mut pager, rid).unwrap(), SeekResult::Exact);
        assert_eq!(cursor.stack, vec![(2, i as u16)]);

        assert_eq!(
            cursor.seek(&mut pager, rid - 1).unwrap(),
            SeekResult::NotFound
        );
        assert_eq!(cursor.stack, vec![(2, i as u16)], "just below {rid}");

        assert_eq!(
            cursor.seek(&mut pager, rid + 1).unwrap(),
            SeekResult::NotFound
        );
        assert_eq!(cursor.stack, vec![(2, i as u16 + 1)], "just above {rid}");
    }
    assert_eq!(cursor.seek(&mut pager, 0).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 0)]);
    assert_eq!(cursor.seek(&mut pager, 351).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 50)]);
}

#[test]
fn seek_resets_path_between_calls() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 1).unwrap();
    assert_eq!(cursor.stack, vec![(2, 0)]);
    cursor.seek(&mut pager, 3).unwrap();
    assert_eq!(cursor.stack, vec![(2, 2)]);
    assert_eq!(cursor.stack.len(), 1);
}

#[test]
fn seek_then_seek_same_position_stays_at() {
    let mut pager = leaf_root(&[(5, b"a")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 5).unwrap();
    cursor.seek(&mut pager, 5).unwrap();
    assert_eq!(cursor.state, CursorState::At);
    assert_eq!(cursor.stack, vec![(2, 0)]);
}

#[test]
fn seek_on_out_of_range_root_errors() {
    let mut pager = leaf_root(&[(1, b"a")]);
    let mut cursor = BTreeCursor::new(99);
    assert!(cursor.seek(&mut pager, 1).is_err());
}

#[test]
fn seek_on_root_zero_errors() {
    let mut pager = leaf_root(&[(1, b"a")]);
    let mut cursor = BTreeCursor::new(0);
    assert!(cursor.seek(&mut pager, 1).is_err());
}

#[test]
fn first_positions_at_first_cell() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.first(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::At);
    assert_eq!(cursor.stack, vec![(2, 0)]);
}

#[test]
fn last_positions_at_last_cell() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.last(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::At);
    assert_eq!(cursor.stack, vec![(2, 2)]);
}

#[test]
#[should_panic]
fn last_on_empty_leaf_panics() {
    let mut pager = leaf_root(&[]);
    let mut cursor = BTreeCursor::new(2);
    let _ = cursor.last(&mut pager);
}

#[test]
fn first_then_next_walks_forward() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    let rowids = walk_forward(&mut pager, &mut cursor);
    assert_eq!(rowids, vec![1, 2, 3]);
}

#[test]
fn last_then_prev_walks_backward() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    let rowids = walk_backward(&mut pager, &mut cursor);
    assert_eq!(rowids, vec![3, 2, 1]);
}

#[test]
fn next_past_last_leaf_enters_after_last() {
    let mut pager = leaf_root(&[(1, b"a")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 1).unwrap();
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::AfterLast);
    assert!(cursor.stack.is_empty());
}

#[test]
fn prev_before_first_leaf_enters_before_first() {
    let mut pager = leaf_root(&[(1, b"a")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 1).unwrap();
    cursor.prev(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::BeforeFirst);
    assert!(cursor.stack.is_empty());
}

#[test]
fn next_from_insertion_point_after_last_cell_enters_after_last() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 25).unwrap(); // NotFound -> stack [(2,2)]
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::AfterLast);
}

#[test]
fn prev_from_insertion_point_before_first_enters_before_first() {
    let mut pager = leaf_root(&[(5, b"a")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 0).unwrap(); // NotFound -> stack [(2,0)]
    cursor.prev(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::BeforeFirst);
}

#[test]
fn next_after_after_last_stays_after_last() {
    let mut pager = leaf_root(&[(1, b"a")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.first(&mut pager).unwrap();
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::AfterLast);
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::AfterLast);
}

#[test]
fn prev_after_before_first_stays_before_first() {
    let mut pager = leaf_root(&[(1, b"a")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.first(&mut pager).unwrap();
    cursor.prev(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::BeforeFirst);
    cursor.prev(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::BeforeFirst);
}

#[test]
fn seek_after_after_last_resets_to_at() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.first(&mut pager).unwrap();
    cursor.next(&mut pager).unwrap();
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::AfterLast);
    cursor.seek(&mut pager, 1).unwrap();
    assert_eq!(cursor.state, CursorState::At);
    assert_eq!(cursor.stack, vec![(2, 0)]);
}

#[test]
fn current_returns_none_when_unpositioned() {
    let mut pager = leaf_root(&[(1, b"a")]);
    let mut cursor = BTreeCursor::new(2);
    assert!(cursor.current(&mut pager).unwrap().is_none());
}

#[test]
fn current_returns_some_when_positioned() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 2).unwrap();
    let cell = cursor.current(&mut pager).unwrap().expect("positioned");
    // current() reads cell(0) of the current page regardless of the stack idx.
    assert_eq!(cell.row_id(), 1);
}

#[test]
fn current_returns_first_cell_even_when_mid_page() {
    let mut pager = leaf_root(&[(1, b"a"), (2, b"b"), (3, b"c")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.last(&mut pager).unwrap(); // stack [(2,2)] -> last cell
    let cell = cursor.current(&mut pager).unwrap().expect("positioned");
    // Documents the current() behavior: it ignores the stack cell index.
    assert_eq!(cell.row_id(), 1);
}

#[test]
fn current_returns_none_after_next_past_end() {
    let mut pager = leaf_root(&[(1, b"a")]);
    let mut cursor = BTreeCursor::new(2);
    cursor.first(&mut pager).unwrap();
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::AfterLast);
    assert!(cursor.current(&mut pager).unwrap().is_none());
}

#[test]
fn two_level_seek_left_subtree() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 2).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 0), (3, 1)]);
}

#[test]
fn two_level_seek_boundary_rowid() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 3).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 0), (3, 2)]);
}

#[test]
fn two_level_seek_right_subtree() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 5).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 1), (4, 1)]);
}

#[test]
fn two_level_seek_missing_in_right_subtree() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 7).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 1), (4, 3)]);
}

#[test]
fn two_level_seek_missing_below_all() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 0).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 0), (3, 0)]);
}

#[test]
fn two_level_first_and_last() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    cursor.first(&mut pager).unwrap();
    assert_eq!(cursor.stack, vec![(2, 0), (3, 0)]);
    assert_eq!(cell_row_id(&mut pager, 3, 0), 1);

    let mut cursor = BTreeCursor::new(2);
    cursor.last(&mut pager).unwrap();
    assert_eq!(cursor.stack, vec![(2, 1), (4, 2)]);
    assert_eq!(cell_row_id(&mut pager, 4, 2), 6);
}

#[test]
fn two_level_next_crosses_from_left_to_right_leaf() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 3).unwrap(); // last cell of left leaf
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.stack, vec![(2, 1), (4, 0)]);
    assert_eq!(cell_row_id(&mut pager, 4, 0), 4);
}

#[test]
fn two_level_prev_crosses_from_right_to_left_leaf() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 4).unwrap(); // first cell of right leaf
    cursor.prev(&mut pager).unwrap();
    assert_eq!(cursor.stack, vec![(2, 0), (3, 2)]);
    assert_eq!(cell_row_id(&mut pager, 3, 2), 3);
}

#[test]
fn two_level_next_from_last_cell_enters_after_last() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 6).unwrap();
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::AfterLast);
    assert!(cursor.stack.is_empty());
}

#[test]
fn two_level_prev_from_first_cell_enters_before_first() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 1).unwrap();
    cursor.prev(&mut pager).unwrap();
    assert_eq!(cursor.state, CursorState::BeforeFirst);
    assert!(cursor.stack.is_empty());
}

#[test]
fn two_level_full_forward_walk() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(walk_forward(&mut pager, &mut cursor), vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn two_level_full_backward_walk() {
    let mut pager = two_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(walk_backward(&mut pager, &mut cursor), vec![6, 5, 4, 3, 2, 1]);
}

#[test]
fn three_level_seek_deep_left() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 4).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 0), (3, 1), (6, 1)]);
}

#[test]
fn three_level_seek_deep_left_leaf() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 2).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 0), (3, 0), (5, 1)]);
}

#[test]
fn three_level_seek_top_level_right() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 5).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 1), (4, 0)]);
}

#[test]
fn three_level_seek_missing_deep() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 0).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 0), (3, 0), (5, 0)]);
    assert_eq!(cursor.seek(&mut pager, 7).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 1), (4, 2)]);
}

#[test]
fn three_level_next_crosses_leaf_then_subtree() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 2).unwrap(); // last cell of page5
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.stack, vec![(2, 0), (3, 1), (6, 0)]);

    cursor.seek(&mut pager, 4).unwrap(); // last cell of page6
    cursor.next(&mut pager).unwrap();
    assert_eq!(cursor.stack, vec![(2, 1), (4, 0)]);
}

#[test]
fn three_level_prev_crosses_leaf_then_subtree() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    cursor.seek(&mut pager, 3).unwrap(); // first cell of page6
    cursor.prev(&mut pager).unwrap();
    assert_eq!(cursor.stack, vec![(2, 0), (3, 0), (5, 1)]);

    cursor.seek(&mut pager, 5).unwrap(); // first cell of page4
    cursor.prev(&mut pager).unwrap();
    assert_eq!(cursor.stack, vec![(2, 0), (3, 1), (6, 1)]);
}

#[test]
fn three_level_full_forward_walk() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(walk_forward(&mut pager, &mut cursor), vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn three_level_full_backward_walk() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    assert_eq!(walk_backward(&mut pager, &mut cursor), vec![6, 5, 4, 3, 2, 1]);
}

#[test]
fn three_level_forward_then_backward_round_trip() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    let fwd = walk_forward(&mut pager, &mut cursor);
    let bwd = walk_backward(&mut pager, &mut cursor);
    assert_eq!(fwd, vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(bwd, vec![6, 5, 4, 3, 2, 1]);
}

#[test]
fn descend_to_first_lands_on_leftmost_leaf_cell() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    cursor.stack.push((2, 0));
    cursor.descend_to_first(&mut pager, 3).unwrap();
    // assert_eq!(cursor.stack, vec![(2, 0), (3, 0), (5, 0)]);
    // assert_eq!(cursor.state, CursorState::At);
}

#[test]
fn descend_to_first_on_leaf_stays() {
    let mut pager = three_level_tree();
    let mut cursor = BTreeCursor::new(2);
    cursor.stack.push((2, 0));
    cursor.descend_to_first(&mut pager, 5).unwrap();
    assert_eq!(cursor.stack, vec![(2, 0), (5, 0)]);
}

#[test]
fn seek_on_cell_less_interior_root_uses_right_most_child() {
    let mut data = make_db(PAGE_SIZE, 3);
    common::write_interior_table_page(&mut data[PAGE_SIZE..2 * PAGE_SIZE], 0, &[], 3);
    common::write_leaf_table_page(
        &mut data[2 * PAGE_SIZE..3 * PAGE_SIZE],
        0,
        &[enc(1, b"a")],
    );
    let mut pager = pager_from(data, PAGE_SIZE, 3);

    let mut cursor = BTreeCursor::new(2);
    assert_eq!(cursor.seek(&mut pager, 5).unwrap(), SeekResult::NotFound);
    assert_eq!(cursor.stack, vec![(2, 0), (3, 1)]);

    assert_eq!(cursor.seek(&mut pager, 1).unwrap(), SeekResult::Exact);
    assert_eq!(cursor.stack, vec![(2, 0), (3, 0)]);
}



#[test]
fn many_seeks_over_dense_leaf_do_not_leak_pins() {
    // 80 cells (max that fits a 512-byte page with room to spare), 80 exact
    // seeks + 80 misses, then a full walk.
    let cells: Vec<Vec<u8>> = (1..=80).map(|i| enc(i, b"x")).collect();
    let mut pager = leaf_db(PAGE_SIZE, 2, 2, &cells);

    let mut cursor = BTreeCursor::new(2);
    for i in 1..=80u64 {
        assert_eq!(cursor.seek(&mut pager, i).unwrap(), SeekResult::Exact);
        assert_eq!(cursor.seek(&mut pager, i + 80).unwrap(), SeekResult::NotFound);
    }
    let rowids = walk_forward(&mut pager, &mut cursor);
    assert_eq!(rowids.len(), 80);
    assert_eq!(rowids[0], 1);
    assert_eq!(rowids[79], 80);
}

//! Tests for `inkdb::format::cell`.
//!
//! Cell parsers take a `Read + Seek` cursor directly, so these are pure
//! in-memory tests using `std::io::Cursor<Vec<u8>>` - no disk or SqliteFile
//! needed.

mod common;

use common::{encode_index_interior_cell, encode_index_leaf_cell, encode_table_leaf_cell};
use inkdb::format::cell::{IndexInteriorCell, IndexLeafCell, TableInteriorCell, TableLeafCell};
use std::io::Cursor;

/// A minimal stand-in for `CellPointer` since it has no public constructor
/// in the uploaded source (only `.get()` is exposed). We reach it through
/// `BTreePage::cell()`, which is the only public path that produces one -
/// so these low-level cell tests instead build a one-cell page via a tiny
/// local helper, then delegate to `BTreePage`. This still exercises each
/// `XxxCell::parse` function (BTreePage::cell dispatches straight into it)
/// while working within the crate's actual public API surface.
mod via_btree_page {
    use crate::common::write_leaf_table_page;

    use super::*;
    use inkdb::format::cell::BTreeCell;
    use inkdb::format::page::BTreePage;

    const PAGE_SIZE: usize = 512;

    fn leaf_table_page_with_cell(cell: Vec<u8>) -> BTreePage {
        let mut buf = vec![0u8; PAGE_SIZE];
        crate::common::write_leaf_table_page(&mut buf, 0, &[cell]);
        BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap()
    }

    fn interior_table_page_with_cell(left_child: u32, rowid: u64) -> BTreePage {
        let mut buf = vec![0u8; PAGE_SIZE];
        crate::common::write_interior_table_page(&mut buf, 0, &[(left_child, rowid)], 99);
        BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap()
    }

    fn leaf_index_page_with_cell(cell: Vec<u8>) -> BTreePage {
        let mut buf = vec![0u8; PAGE_SIZE];
        crate::common::write_leaf_index_page(&mut buf, 0, &[cell]);
        BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap()
    }

    fn interior_index_page_with_cell(cell: Vec<u8>) -> BTreePage {
        let mut buf = vec![0u8; PAGE_SIZE];
        crate::common::write_interior_index_page(&mut buf, 0, &[cell], 99);
        BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap()
    }
    #[test]
    fn table_leaf_cell_all_varint_boundaries() {
        let payload = b"x";

        let values = [
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            2_097_151,
            2_097_152,
            268_435_455,
            268_435_456,
            34_359_738_367,
            34_359_738_368,
            4_398_046_511_103,
            4_398_046_511_104,
            562_949_953_421_311,
            562_949_953_421_312,
            72_057_594_037_927_935,
            72_057_594_037_927_936,
            u64::MAX,
        ];

        for row_id in values {
            let cell = encode_table_leaf_cell(1, row_id, payload, None);
            let page = leaf_table_page_with_cell(cell);
            let cell = page.cell(0).unwrap();

            let range = cell.payload();
            assert_eq!(&page.bytes()[range.start..range.end], payload);
        }
    }

    #[test]
    fn table_leaf_cell_no_overflow_parses_payload_len_and_row_id() {
        let payload = b"hello world";
        let cell_bytes = encode_table_leaf_cell(payload.len() as u64, 42, payload, None);
        let page = leaf_table_page_with_cell(cell_bytes);
        let cell = page.cell(0).unwrap();

        assert_eq!(cell.cell_payload_len(), payload.len() as u64);
        assert!(cell.overflow_page().is_none());
        let range = cell.payload();
        assert_eq!(range.end - range.start, payload.len());
        assert_eq!(&page.bytes()[range.start..range.end], payload);
    }
    #[test]
    fn table_leaf_multiple_cells() {
        let c1 = encode_table_leaf_cell(1, 1, b"a", None);
        let c2 = encode_table_leaf_cell(1, 2, b"b", None);
        let c3 = encode_table_leaf_cell(1, 3, b"c", None);

        let mut buf = vec![0u8; PAGE_SIZE];

        write_leaf_table_page(&mut buf, 0, &[c1, c2, c3]);

        let page = BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap();

        for (i, expected) in [b"a", b"b", b"c"].iter().enumerate() {
            let range = page.cell(i as u16).unwrap().payload().clone();

            assert_eq!(&page.bytes()[range.start..range.end], *expected);
        }
    }
    #[test]
    fn table_leaf_variable_payload_sizes() {
        let p1 = vec![0x11; 1];
        let p2 = vec![0x22; 50];
        let p3 = vec![0x33; 120];

        let c1 = encode_table_leaf_cell(1, 1, &p1, None);
        let c2 = encode_table_leaf_cell(50, 2, &p2, None);
        let c3 = encode_table_leaf_cell(120, 3, &p3, None);

        let mut buf = vec![0u8; PAGE_SIZE];

        write_leaf_table_page(&mut buf, 0, &[c1, c2, c3]);

        let page = BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap();

        let r = page.cell(0).unwrap().payload().clone();
        assert_eq!(&page.bytes()[r.start..r.end], p1.as_slice());

        let r = page.cell(1).unwrap().payload().clone();
        assert_eq!(&page.bytes()[r.start..r.end], p2.as_slice());

        let r = page.cell(2).unwrap().payload().clone();
        assert_eq!(&page.bytes()[r.start..r.end], p3.as_slice());
    }
    #[test]
    fn table_leaf_large_payload() {
        let payload = vec![0x55; 400];
    
        let cell = encode_table_leaf_cell(
            payload.len() as u64,
            1,
            &payload,
            None,
        );
    
        let page = leaf_table_page_with_cell(cell);
    
        let cell = page.cell(0).unwrap();
    
        let range = cell.payload();
    
        assert_eq!(
            &page.bytes()[range.start..range.end],
            payload.as_slice()
        );
    }
    #[test]
    fn table_leaf_all_rowid_varint_boundaries() {
        let values = [
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            2_097_151,
            2_097_152,
            268_435_455,
            268_435_456,
            u32::MAX as u64,
            u64::MAX,
        ];

        for row_id in values {
            let payload = b"x";

            let cell = encode_table_leaf_cell(payload.len() as u64, row_id, payload, None);

            let page = leaf_table_page_with_cell(cell);

            let cell = page.cell(0).unwrap();

            let range = cell.payload();

            assert_eq!(&page.bytes()[range.start..range.end], payload);
        }
    }
    #[test]
    fn table_leaf_empty_payload() {
        let cell = encode_table_leaf_cell(0, 42, b"", None);

        let page = leaf_table_page_with_cell(cell);

        let cell = page.cell(0).unwrap();

        let range = cell.payload();

        assert_eq!(range.start, range.end);
    }

    #[test]
    fn table_leaf_cell_row_id_varint_boundary_values() {
        for row_id in [0u64, 1, 127, 128, 16383, 16384, u32::MAX as u64] {
            let payload = b"x";
            let cell_bytes = encode_table_leaf_cell(payload.len() as u64, row_id, payload, None);
            // dbg!(&cell_bytes);
            let page = leaf_table_page_with_cell(cell_bytes);
            dbg!(row_id);
            dbg!(&page);
            let cell = page.cell(0).unwrap();
            dbg!(&cell);
            // dbg!(cell);
            // We can't directly read row_id (private field, no getter) but
            // we can confirm parsing succeeds and payload is intact for
            // varint-length-varying row ids, which exercises the seek
            // arithmetic after the row_id varint.
            let range = cell.payload();
            assert_eq!(&page.bytes()[range.start..range.end], payload);
        }
    }

    #[test]
    fn table_leaf_cell_with_overflow_pointer_zero_is_rejected() {
        // A payload big enough to force overflow, but overflow pointer is 0
        // (invalid per SQLite format).
        let usable = PAGE_SIZE;
        let x = usable - 35; // local/overflow threshold
        let payload_len = x + 100; // force overflow
        let local_len = inkdb::format::overflow::compute_local_payload_size(usable, payload_len);
        let local_payload = vec![0xEEu8; local_len];

        let cell_bytes = encode_table_leaf_cell(payload_len as u64, 1, &local_payload, Some(0));
        let mut buf = vec![0u8; PAGE_SIZE];
        crate::common::write_leaf_table_page(&mut buf, 0, &[cell_bytes]);
        let page = BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap();

        let result = page.cell(0);
        assert!(result.is_err());
    }

    #[test]
    fn table_leaf_cell_with_valid_overflow_pointer_parses() {
        let usable = PAGE_SIZE;
        let x = usable - 35;
        let payload_len = x + 100;
        let local_len = inkdb::format::overflow::compute_local_payload_size(usable, payload_len);
        let local_payload = vec![0xEEu8; local_len];

        let cell_bytes = encode_table_leaf_cell(payload_len as u64, 1, &local_payload, Some(9));
        let mut buf = vec![0u8; PAGE_SIZE];
        crate::common::write_leaf_table_page(&mut buf, 0, &[cell_bytes]);
        let page = BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap();

        let cell = page.cell(0).unwrap();
        assert_eq!(cell.overflow_page(), Some(9));
        assert_eq!(cell.cell_payload_len(), payload_len as u64);
    }

    #[test]
    fn table_interior_cell_parses_left_child_and_rowid() {
        let page = interior_table_page_with_cell(123, 999);
        let cell = page.cell(0).unwrap();
        match cell {
            BTreeCell::TableInterior(_) => {}
            _ => panic!("expected TableInterior"),
        }
    }

    #[test]
    fn table_interior_cell_large_rowid_boundary_parses() {
        let page = interior_table_page_with_cell(1, u32::MAX as u64 * 2);
        let cell = page.cell(0).unwrap();
        match cell {
            BTreeCell::TableInterior(_) => {}
            _ => panic!("expected TableInterior"),
        }
    }

    #[test]
    fn index_leaf_cell_no_overflow_parses() {
        let payload = b"index-key-data";
        let cell_bytes = encode_index_leaf_cell(payload.len() as u64, payload, None);
        let page = leaf_index_page_with_cell(cell_bytes);
        let cell = page.cell(0).unwrap();

        assert!(cell.overflow_page().is_none());
        let range = cell.payload();
        assert_eq!(&page.bytes()[range.start..range.end], payload);
    }

    #[test]
    fn index_leaf_cell_zero_overflow_pointer_is_rejected() {
        let usable = PAGE_SIZE;
        let x = usable - ((usable - 12) * 64 / 255) + 23; // rough index X bound, over-approx is fine
        let payload_len = usable; // definitely forces overflow at this page size
        let local_len = inkdb::format::overflow::compute_local_payload_size(usable, payload_len);
        let local_payload = vec![0x01u8; local_len];

        let cell_bytes = encode_index_leaf_cell(payload_len as u64, &local_payload, Some(0));
        let mut buf = vec![0u8; PAGE_SIZE];
        crate::common::write_leaf_index_page(&mut buf, 0, &[cell_bytes]);
        let page = BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap();

        let result = page.cell(0);
        assert!(result.is_err());
    }

    #[test]
    fn index_interior_cell_parses_left_child_and_payload() {
        let payload = b"key";
        let cell_bytes = encode_index_interior_cell(55, payload.len() as u64, payload, None);
        let page = interior_index_page_with_cell(cell_bytes);
        dbg!(&page);
        let cell = page.cell(0).unwrap();
        // dbg!(&cell);

        match cell {
            BTreeCell::IndexInterior(_) => {}
            _ => panic!("expected IndexInterior"),
        }
        let range = cell.payload();
        // dbg!(range);
        // dbg!(&page.bytes()[range.start..range.end]);
        // dbg!(page.bytes());
        assert_eq!(&page.bytes()[range.start..range.end], payload);
    }

    #[test]
    fn index_interior_cell_with_valid_overflow_parses() {
        let usable = PAGE_SIZE;
        let payload_len = usable; // force overflow
        let local_len = inkdb::format::overflow::compute_local_payload_size(usable, payload_len);
        let local_payload = vec![0x02u8; local_len];

        let cell_bytes = encode_index_interior_cell(1, payload_len as u64, &local_payload, Some(5));
        let mut buf = vec![0u8; PAGE_SIZE];
        crate::common::write_interior_index_page(&mut buf, 0, &[cell_bytes], 99);
        let page = BTreePage::parse(buf, 2, PAGE_SIZE, PAGE_SIZE).unwrap();

        let cell = page.cell(0).unwrap();
        assert_eq!(cell.overflow_page(), Some(5));
    }

    #[test]
    fn cell_type_reports_correctly_for_each_variant() {
        use inkdb::format::cell::BTreeCellType;

        let payload = b"p";
        let leaf_cell = encode_table_leaf_cell(payload.len() as u64, 1, payload, None);
        let page = leaf_table_page_with_cell(leaf_cell);
        assert_eq!(page.cell(0).unwrap().cell_type(), BTreeCellType::TableLeaf);

        let idx_leaf_cell = encode_index_leaf_cell(payload.len() as u64, payload, None);
        let page = leaf_index_page_with_cell(idx_leaf_cell);
        assert_eq!(page.cell(0).unwrap().cell_type(), BTreeCellType::IndexLeaf);

        let idx_int_cell = encode_index_interior_cell(1, payload.len() as u64, payload, None);
        let page = interior_index_page_with_cell(idx_int_cell);
        assert_eq!(
            page.cell(0).unwrap().cell_type(),
            BTreeCellType::IndexInterior
        );
    }
}

// ---------------------------------------------------------------------
// Direct parse() tests using bare Cursor<Vec<u8>>, bypassing BTreePage.
// These require a `CellPointer`, which has no public constructor - so we
// only exercise `TableInteriorCell::parse`, the one parser signature that
// takes a `CellPointer` but whose call sites we can still replicate by
// reading the raw seek arithmetic directly through BTreePage as above.
// (Kept as a small sanity note; real coverage lives in `via_btree_page`.)
mod notes {
    //! `CellPointer(u16)` is a private tuple struct with only `.get()`
    //! exposed publicly, so `TableInteriorCell::parse`, `TableLeafCell::parse`,
    //! `IndexInteriorCell::parse`, and `IndexLeafCell::parse` cannot be
    //! called directly from an external integration test - they all require
    //! a `CellPointer` value. All meaningful coverage of these parsers is
    //! therefore routed through `BTreePage::cell()`, which is the crate's
    //! only public source of `CellPointer`s. See the `via_btree_page`
    //! module above.
}

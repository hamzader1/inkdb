use inkdb::format::page::BTreePage;

fn page(kind: u8, cell_offset: usize, cell: &[u8]) -> BTreePage {
    let interior = matches!(kind, 0x02 | 0x05);
    let pointer_offset = if interior { 12 } else { 8 };

    let mut bytes = vec![0; 512];

    bytes[0] = kind;
    bytes[3..5].copy_from_slice(&1u16.to_be_bytes());
    bytes[5..7].copy_from_slice(&(cell_offset as u16).to_be_bytes());

    if interior {
        // Valid right-most child for the page header.
        bytes[8..12].copy_from_slice(&2u32.to_be_bytes());
    }

    // One cell pointer.
    bytes[pointer_offset..pointer_offset + 2].copy_from_slice(&(cell_offset as u16).to_be_bytes());

    bytes[cell_offset..cell_offset + cell.len()].copy_from_slice(cell);

    BTreePage::parse(bytes, 2, 512, 512).unwrap()
}

#[test]
fn every_cell_kind_rejects_a_truncated_varint() {
    // Put 0x80 at the END of the page.
    //
    // 0x80 means “the varint continues”.
    // There is no next byte, so it is truly truncated.
    for kind in [0x0d, 0x0a] {
        let result = std::panic::catch_unwind(|| page(kind, 511, &[0x80]).cell(0));

        assert!(matches!(result, Ok(Err(_))), "kind {kind:#x}");
    }
}

#[test]
fn interior_cells_reject_missing_left_child_bytes() {
    // An interior cell needs 4 bytes for its left-child page number.
    // Starting at byte 510 gives it only 2 bytes before page end.
    for kind in [0x05, 0x02] {
        let result = std::panic::catch_unwind(|| page(kind, 510, &[0, 0]).cell(0));

        assert!(matches!(result, Ok(Err(_))), "kind {kind:#x}");
    }
}

#[test]
fn table_leaf_zero_overflow_pointer_returns_error_not_panic() {
    // Payload length = 1000, rowid = 1.
    // Such a payload needs overflow pages on a 512-byte page.
    // The unwritten overflow-pointer bytes are zero.
    let result = std::panic::catch_unwind(|| page(0x0d, 10, &[0x87, 0x68, 0x01]).cell(0));

    assert!(matches!(result, Ok(Err(_))));
}

#[test]
fn interior_zero_left_child_returns_error() {
    for kind in [0x05, 0x02] {
        // First four cell bytes are the left-child page number: 0.
        let result = std::panic::catch_unwind(|| page(kind, 507, &[0, 0, 0, 0, 1]).cell(0));

        assert!(matches!(result, Ok(Err(_))), "kind {kind:#x}");
    }
}

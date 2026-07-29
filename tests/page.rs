use inkdb::format::page::BTreePage;

#[test]
fn page_mutations_return_errors() {
    let mut page = vec![0; 512];
    page[0] = 0x0d;
    page[5..7].copy_from_slice(&512u16.to_be_bytes());
    for offset in [0, 7] {
        let mut bytes = page.clone();
        bytes[offset] = if offset == 0 { 0xff } else { 61 };
        assert!(BTreePage::parse(bytes, 2, 512, 512).is_err());
    }
    let mut interior = vec![0; 512];
    interior[0] = 0x05;
    interior[5..7].copy_from_slice(&512u16.to_be_bytes());
    assert!(BTreePage::parse(interior, 2, 512, 512).is_err());
}

fn leaf() -> Vec<u8> {
    let mut bytes = vec![0; 512];
    bytes[0] = 0x0d;
    bytes[5..7].copy_from_slice(&512u16.to_be_bytes());
    bytes
}
#[test]
fn rejects_each_cell_pointer_region() {
    for pointer in [0u16, 1, 7, 8, 9, 512] {
        let mut bytes = leaf();
        bytes[3..5].copy_from_slice(&1u16.to_be_bytes());
        bytes[8..10].copy_from_slice(&pointer.to_be_bytes());
        assert!(BTreePage::parse(bytes, 2, 512, 512).is_err());
    }
}
#[test]
fn rejects_pointer_array_past_usable_size() {
    let mut bytes = leaf();
    bytes[3..5].copy_from_slice(&253u16.to_be_bytes());
    assert!(BTreePage::parse(bytes, 2, 512, 512).is_err());
}
#[test]
fn rejects_cell_content_area_outside_usable_size() {
    let mut bytes = leaf();
    bytes[5..7].copy_from_slice(&513u16.to_be_bytes());
    assert!(BTreePage::parse(bytes, 2, 512, 512).is_err());
}
#[test]
fn rejects_all_short_page_headers() {
    for length in 0..8 {
        assert!(BTreePage::parse(vec![0x0d; length], 2, 512, 512).is_err());
    }
}
#[test]
fn page_one_requires_header_at_offset_one_hundred() {
    let mut bytes = vec![0; 512];
    bytes[100] = 0x0d;
    bytes[105..107].copy_from_slice(&512u16.to_be_bytes());
    assert!(BTreePage::parse(bytes, 1, 512, 512).is_ok());
}
#[test]
fn page_one_invalid_type_at_offset_one_hundred_is_rejected() {
    let mut bytes = vec![0; 512];
    bytes[100] = 0xff;
    assert!(BTreePage::parse(bytes, 1, 512, 512).is_err());
}

use inkdb::format::overflow::OverflowPageRef;
use std::fs;

#[test]
fn overflow_fixture_is_present() {
    assert!(fs::metadata("tests/fixtures/overflow.db").unwrap().len() > 512);
}

#[test]
fn overflow_page_reference_validates_all_short_buffers() {
    for length in 0..512 {
        let bytes = vec![0; length];
        let result = std::panic::catch_unwind(|| OverflowPageRef::new(&bytes, 512));
        assert!(matches!(result, Ok(Err(_))), "length {length}");
    }
}

#[test]
fn complete_overflow_reference_exposes_next_page_and_payload() {
    let mut bytes = vec![0; 512];
    bytes[..4].copy_from_slice(&3u32.to_be_bytes());
    bytes[4..].fill(0xab);
    let reference = OverflowPageRef::new(&bytes, 512).unwrap();
    assert_eq!(reference.next, 3);
    assert_eq!(reference.data, &[0xab; 508]);
}

#[test]
fn overflow_reference_rejects_usable_size_past_buffer() {
    let bytes = vec![0; 512];
    let result = std::panic::catch_unwind(|| OverflowPageRef::new(&bytes, 513));
    assert!(matches!(result, Ok(Err(_))));
}
#[test]
fn overflow_reference_preserves_zero_next_page() {
    let bytes = vec![0; 512];
    let reference = OverflowPageRef::new(&bytes, 512).unwrap();
    assert_eq!(reference.next, 0);
    assert_eq!(reference.data.len(), 508);
}
#[test]
fn overflow_reference_preserves_known_payload_bytes() {
    let mut bytes = vec![0; 512];
    for (index, byte) in bytes[4..].iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    let reference = OverflowPageRef::new(&bytes, 512).unwrap();
    assert_eq!(reference.data[0], 0);
    assert_eq!(reference.data[250], 250);
    assert_eq!(reference.data[251], 0);
}

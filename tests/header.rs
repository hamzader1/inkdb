use inkdb::format::header::SqliteDatabaseHeader;
use std::fs;
use std::io::Cursor;

// #[test]
// fn simple_fixture_has_a_complete_sqlite_header() {
//     let bytes = fs::read("tests/fixtures/simple.db").unwrap();
//     assert!(bytes.len() >= 100);
//     assert_eq!(&bytes[..16], b"SQLite format 3\0");
// }

// #[test]
// fn corrupted_header_mutations_return_errors() {
//     let original = fs::read("tests/fixtures/simple.db").unwrap();
//     for (offset, value) in [
//         (0, 0),
//         (16, 1),
//         (21, 63),
//         (22, 31),
//         (23, 31),
//         (56, 4),
//         (72, 1),
//     ] {
//         let mut bytes = original.clone();
//         bytes[offset] = value;
//         assert!(SqliteDatabaseHeader::parse(&mut Cursor::new(bytes)).is_err());
//     }
//     for length in 0..100 {
//         assert!(
//             SqliteDatabaseHeader::parse(&mut Cursor::new(original[..length].to_vec())).is_err()
//         );
//     }
// }

// fn simple_bytes() -> Vec<u8> {
//     fs::read("tests/fixtures/simple.db").unwrap()
// }
// fn parse(bytes: Vec<u8>) -> bool {
//     SqliteDatabaseHeader::parse(&mut Cursor::new(bytes)).is_err()
// }

// #[test]
// fn rejects_each_invalid_payload_fraction() {
//     for (offset, value) in [(21, 63), (22, 31), (23, 31)] {
//         let mut bytes = simple_bytes();
//         bytes[offset] = value;
//         assert!(parse(bytes));
//     }
// }
// #[test]
// fn rejects_non_power_of_two_page_sizes() {
//     for size in [513u16, 768, 1025, 16383] {
//         let mut bytes = simple_bytes();
//         bytes[16..18].copy_from_slice(&size.to_be_bytes());
//         assert!(parse(bytes));
//     }
// }
// #[test]
// fn rejects_page_sizes_below_sqlite_minimum() {
//     for size in [0u16, 2, 511] {
//         let mut bytes = simple_bytes();
//         bytes[16..18].copy_from_slice(&size.to_be_bytes());
//         assert!(parse(bytes));
//     }
// }
// #[test]
// fn rejects_invalid_text_encodings() {
//     for encoding in [0u32, 4, u32::MAX] {
//         let mut bytes = simple_bytes();
//         bytes[56..60].copy_from_slice(&encoding.to_be_bytes());
//         assert!(parse(bytes));
//     }
// }
// #[test]
// fn rejects_every_reserved_expansion_byte() {
//     for offset in 72..92 {
//         let mut bytes = simple_bytes();
//         bytes[offset] = 1;
//         assert!(parse(bytes), "offset {offset}");
//     }
// }
// #[test]
// fn valid_fixture_header_parses() {
//     assert!(!parse(simple_bytes()));
// }

// //! Tests for `inkdb::vfs::cursor::FileCursor`.

// mod common;

// use common::MemFile;
// use inkdb::vfs::cursor::FileCursor;

// #[test]
// fn new_cursor_starts_at_offset_zero() {
//     let file = MemFile::new(vec![0x99, 0x00]);
//     let mut cur = FileCursor::new(&file);
//     assert_eq!(cur.read_next_u8().unwrap(), 0x99);
// }

// #[test]
// fn read_next_exact_advances_by_buffer_length() {
//     let file = MemFile::new(vec![1, 2, 3, 4, 5]);
//     let mut cur = FileCursor::new(&file);
//     let mut buf = [0u8; 3];
//     cur.read_next_exact(&mut buf).unwrap();
//     assert_eq!(buf, [1, 2, 3]);
//     let mut buf2 = [0u8; 2];
//     cur.read_next_exact(&mut buf2).unwrap();
//     assert_eq!(buf2, [4, 5]);
// }

// #[test]
// fn read_next_exact_zero_length_buffer_does_not_advance() {
//     let file = MemFile::new(vec![1, 2]);
//     let mut cur = FileCursor::new(&file);
//     let mut buf: [u8; 0] = [];
//     cur.read_next_exact(&mut buf).unwrap();
//     assert_eq!(cur.read_next_u8().unwrap(), 1);
// }

// #[test]
// fn multiple_cursors_over_same_file_are_independent() {
//     let file = MemFile::new(vec![10, 20, 30, 40]);
//     let mut cur_a = FileCursor::new(&file);
//     let mut cur_b = FileCursor::with_offset(&file, 2);

//     assert_eq!(cur_a.read_next_u8().unwrap(), 10);
//     assert_eq!(cur_b.read_next_u8().unwrap(), 30);
//     // cur_a unaffected by cur_b's reads
//     assert_eq!(cur_a.read_next_u8().unwrap(), 20);
// }

// #[test]
// fn reading_array_larger_than_remaining_data_errors() {
//     let file = MemFile::new(vec![1, 2, 3]);
//     let mut cur = FileCursor::new(&file);
//     let result = cur.read_next_arrary::<10>();
//     assert!(result.is_err());
// }

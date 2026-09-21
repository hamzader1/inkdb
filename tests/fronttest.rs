#[test]
fn front_insert_stable() {
    use inkdb::storage::page::{BTreePage, BTreePageType};
    let ps = 4096usize;
    let mut buf = vec![0u8; ps];
    let mut p = BTreePage::new_from_raw_bytes(7, BTreePageType::LeafIndex, &mut buf[..], ps, ps).unwrap();
    for i in 0..300u16 {
        let cell = vec![0x06, 0x03, 0x01, 0x02, i as u8, 0x11, 0x22];
        p.insert_cell(&cell, 0).unwrap();
        assert_eq!(p.no_of_cells().unwrap(), i + 1);
        assert_eq!(p.page_type().unwrap(), BTreePageType::LeafIndex);
    }
    for i in 0..300u16 {
        let got = p.cell_bytes_as_ref(i).unwrap().to_vec();
        assert_eq!(got.len(), 7, "cell {}", i);
    }
}

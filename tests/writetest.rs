#[test]
fn btree_index_split_roundtrip() {
    use inkdb::pager::pager::Pager;
    use inkdb::record::Value;
    use inkdb::record::tuple::Tuple;
    use inkdb::storage::btree::BTree;
    use inkdb::storage::cell::Encode;
    use inkdb::vfs::disk::DiskVfs;
    use inkdb::vfs::{InkOptions, Vfs};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(100);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("inkdb-split-{}-{}.db", std::process::id(), n));
    let ps = 4096usize;
    let mut vfs = DiskVfs;
    let source = vfs.open(&path, InkOptions::all()).unwrap();
    inkdb::vfs::file::InkFile::set_len(&source, ps * 4).unwrap();
    let header = inkdb::pager::pager::HeaderCache::new(
        ps as u32,
        ps as u32,
        4,
        0,
        0,
        100,
        inkdb::db::header::DbFormat::Sqlite,
    );
    let mut pager = Pager::with_cache(vfs, source, header, 4096).unwrap();
    pager.start_transaction();
    let root = pager.allocate_new_page().unwrap();
    {
        use inkdb::storage::page::{BTreePage, BTreePageType};
        let mut g = pager.get_mut(root).unwrap();
        BTreePage::new_from_raw_bytes(
            root,
            BTreePageType::LeafIndex,
            g.bytes_as_mut_unchecked(),
            ps,
            ps,
            0,
        )
        .unwrap();
    }
    for i in 0..2000u64 {
        let rec = vec![Value::Integer(i as i64), Value::Integer(i as i64)];
        let bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&rec));
        let key = Value::Tuple(vec![Value::Integer(i as i64), Value::Integer(i as i64)].into());
        if let Err(e) = BTree::new(root, &mut pager).insert(&key, bytes) {
            panic!("row {}: {:?}", i, e);
        }
    }
    pager.commit().unwrap();
    let _ = std::fs::remove_file(&path);
}

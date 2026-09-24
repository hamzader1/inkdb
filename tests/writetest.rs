fn dump_tree(
    pager: &mut inkdb::pager::pager::Pager<inkdb::vfs::disk::DiskVfs>,
    root: u32,
) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(p) = stack.pop() {
        if !seen.insert(p) || seen.len() > 40 {
            continue;
        }
        let Ok(g) = pager.get(p) else {
            continue;
        };
        let bytes = g.bytes().to_vec();
        let t = bytes[if p == 1 { 100 } else { 0 }];
        out.push((p, bytes));
        if t == 2 || t == 5 {
            let n = u16::from_be_bytes([
                out.last().unwrap().1[if p == 1 { 103 } else { 3 }],
                out.last().unwrap().1[if p == 1 { 104 } else { 4 }],
            ]) as usize;
            let base = if p == 1 { 112 } else { 12 };
            for i in 0..n.min(200) {
                let o = base + 2 * i;
                let b = &out.last().unwrap().1;
                if o + 4 > b.len() {
                    break;
                }
                let ptr = u16::from_be_bytes([b[o], b[o + 1]]) as usize;
                if ptr + 4 <= b.len() {
                    let child = u32::from_be_bytes([b[ptr], b[ptr + 1], b[ptr + 2], b[ptr + 3]]);
                    if child != 0 && child < 100000 {
                        stack.push(child);
                    }
                }
            }
            let b = &out.last().unwrap().1;
            let ro = if p == 1 { 108 } else { 8 };
            if ro + 4 <= b.len() {
                let r = u32::from_be_bytes([b[ro], b[ro + 1], b[ro + 2], b[ro + 3]]);
                if r != 0 && r < 100000 {
                    stack.push(r);
                }
            }
        }
    }
    out.sort_by_key(|x| x.0);
    out
}

#[test]
fn btree_index_split_roundtrip() {
    use inkdb::pager::pager::Pager;
    use inkdb::record::Value;
    use inkdb::record::tuple::Tuple;
    use inkdb::storage::btree::BTree;
    use inkdb::storage::cell::Encode;
    use inkdb::vfs::disk::DiskVfs;
    use inkdb::vfs::{SqliteOptions, Vfs};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(100);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("inkdb-split-{}-{}.db", std::process::id(), n));
    let ps = 4096usize;
    let mut vfs = DiskVfs;
    let source = vfs.open(&path, SqliteOptions::all()).unwrap();
    inkdb::vfs::file::SqliteFile::set_len(&source, ps * 4).unwrap();
    let header = inkdb::pager::pager::HeaderCache::new(ps as u32, ps as u32, 4, 0, 0);
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
        )
        .unwrap();
    }
    for i in 0..2000u64 {
        let snap_before = if i == 519 {
            Some(dump_tree(&mut pager, root))
        } else {
            None
        };
        let rec = vec![Value::Integer(i as i64), Value::Integer(i as i64)];
        let mut bytes = Encode::encode_index_leaf_cell(Tuple::serialize(&rec));
        let key = Value::Tuple(vec![Value::Integer(i as i64), Value::Integer(i as i64)]);
        if let Err(e) = BTree::new(root, &mut pager).insert(&key, &mut bytes) {
            eprintln!("ROW519-FAIL {:?}", e);
            if let Some(before) = snap_before {
                let after = dump_tree(&mut pager, root);
                for (b, a) in before.iter().zip(after.iter()) {
                    if b.1 != a.1 {
                        eprintln!(
                            "DIFF page={} before={:x?} after={:x?}",
                            b.0,
                            &b.1[..16],
                            &a.1[..16]
                        );
                    }
                }
            }
            panic!("row {}", i);
        }
        {
            let g = pager.get(root).unwrap();
            let pg =
                inkdb::storage::page::BTreePage::new(root, 4096, 4096, g.bytes()).unwrap();
            if pg.page_type().unwrap() as u8 == 2 || pg.page_type().is_err() {
                let r = pg.right_most_ptr();
                static LASTH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let mut h: u64 = 0;
                for b in g.bytes()[..16].iter() {
                    h = h.wrapping_shl(8) | (*b as u64);
                }
                let prev = LASTH.swap(h, std::sync::atomic::Ordering::SeqCst);
                if h != prev && prev != 0 {
                    eprintln!(
                        "HDR-CHANGE row={} {:x?} (rmp={:?})",
                        i,
                        &g.bytes()[..16],
                        r
                    );
                }
                let r = r.unwrap();
                static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let prev = LAST.swap(r.unwrap_or(0), std::sync::atomic::Ordering::SeqCst);
                if r != Some(prev) && prev != 0 {
                    eprintln!("RMP-CHANGE row={} {:?} -> {:?}", i, Some(prev), r);
                }
            }
        }
        if let Err(e) = BTree::new(root, &mut pager).insert(&key, &mut bytes) {
            eprintln!("FAIL row={} err={:?}", i, e);
            let mut stack = vec![root];
            let mut seen = std::collections::HashSet::new();
            while let Some(p) = stack.pop() {
                if !seen.insert(p) || seen.len() > 30 {
                    continue;
                }
                let mut g = match pager.get_mut(p) {
                    Ok(g) => g,
                    Err(e) => {
                        eprintln!("  page={} GET ERR {:?}", p, e);
                        continue;
                    }
                };
                let pg = match inkdb::storage::page::BTreePage::new(
                    p,
                    4096,
                    4096,
                    g.bytes_as_mut_unchecked(),
                ) {
                    Ok(pg) => pg,
                    Err(e) => {
                        eprintln!("  page={} PARSE ERR {:?}", p, e);
                        continue;
                    }
                };
                let t = pg.page_type().unwrap() as u8;
                let n = pg.no_of_cells().unwrap();
                eprintln!("  page={} t={} n={} rmp={:?}", p, t, n, pg.right_most_ptr());
                let mut kids = vec![];
                for c in 0..n {
                    match pg.cell(c) {
                        Ok(_) => {
                            let bytes = pg.cell_bytes_as_ref(c).unwrap().to_vec();
                            eprintln!(
                                "  page={} t={} cell{} len={} bytes={:x?}",
                                p,
                                t,
                                c,
                                bytes.len(),
                                &bytes[..bytes.len().min(10)]
                            );
                        }
                        Err(e) => eprintln!("  page={} t={} cell{} ERR {:?}", p, t, c, e),
                    }
                }
                if t == 2 || t == 5 {
                    for c in 0..n {
                        kids.push(pg.cell(c).unwrap());
                    }
                    for k in kids {
                        use inkdb::storage::cell::BTreeCell;
                        match k {
                            BTreeCell::IndexInterior(x) => stack.push(x.left_child),
                            BTreeCell::TableInterior(x) => stack.push(x.left_child),
                            _ => {}
                        }
                    }
                    if let Ok(Some(r)) = pg.right_most_ptr() {
                        stack.push(r);
                    }
                }
            }
            panic!("row {}: {:?}", i, e);
        }
    }
    pager.commit().unwrap();
    let _ = std::fs::remove_file(&path);
}

#[path = "common.rs"]
mod common;

use common::*;
use std::collections::{HashMap, HashSet};

/// Walk the index tree from root, count references per page.
/// Prints every page referenced more than once with its parents.
fn walk_refcounts(db: &mut inkdb::db::Database<inkdb::vfs::disk::DiskVfs>, root: u32) {
    use inkdb::storage::cell::BTreeCell;
    use inkdb::storage::page::BTreePage;
    let ps = db.pager.page_size();
    let us = db.pager.usable_size();
    let mut counts: HashMap<u32, u32> = HashMap::new();
    let mut parents: HashMap<u32, Vec<(u32, u16)>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    while let Some(pn) = stack.pop() {
        if pn == 0 || !seen.insert(pn) {
            continue;
        }
        if seen.len() > 100000 {
            eprintln!("walk runaway");
            break;
        }
        let Ok(guard) = db.pager.get(pn) else {
            eprintln!("walk: page {pn} unreadable");
            continue;
        };
        let Ok(page) = BTreePage::new(pn, ps, us, guard.bytes_as_ref()) else {
            eprintln!("walk: page {pn} bad header");
            continue;
        };
        let Ok(t) = page.page_type() else { continue };
        if t.is_leaf() {
            continue;
        }
        let Ok(n) = page.no_of_cells() else { continue };
        for i in 0..n {
            let Ok(cell) = page.cell(i) else { continue };
            let lc = match &cell {
                BTreeCell::IndexInterior(x) => x.left_child,
                BTreeCell::TableInterior(x) => x.left_child,
                _ => continue,
            };
            *counts.entry(lc).or_insert(0) += 1;
            parents.entry(lc).or_default().push((pn, i));
            stack.push(lc);
        }
        if let Ok(Some(r)) = page.right_most_ptr() {
            *counts.entry(r).or_insert(0) += 1;
            parents.entry(r).or_default().push((pn, n));
            stack.push(r);
        }
    }
    let mut multi: Vec<_> = counts.iter().filter(|(_, c)| **c > 1).collect();
    multi.sort_unstable();
    eprintln!("walk: {} pages, {} multi-referenced", seen.len(), multi.len());
    for (p, c) in multi.iter().take(10) {
        eprintln!("walk: page {p} x{c} parents={:?}", parents.get(p));
    }
    // Neighborhood dump of the first dup parent: full slot list with keys
    // plus the dup page key range. Saved into the pager for reading here.
    if let Some((p, _)) = multi.first() {
        if let Some(pars) = parents.get(p) {
            let (pp, _) = pars[0];
            dump_parent(db, pp);
            dump_page_keys(db, **p);
        }
    }
}

fn dump_parent(db: &mut inkdb::db::Database<inkdb::vfs::disk::DiskVfs>, pp: u32) {
    use inkdb::storage::cell::BTreeCell;
    use inkdb::storage::page::BTreePage;
    let ps = db.pager.page_size();
    let us = db.pager.usable_size();
    let guard = db.pager.get(pp).unwrap();
    let page = BTreePage::new(pp, ps, us, guard.bytes_as_ref()).unwrap();
    let n = page.no_of_cells().unwrap();
    eprintln!("dump parent {pp}: kind={:?} cells={n} rmp={:?}", page.page_type().unwrap(), page.right_most_ptr().unwrap());
    for i in 0..n {
        let cell = page.cell(i).unwrap();
        match &cell {
            BTreeCell::IndexInterior(x) => {
                let key = page.record_of(&cell, &mut db.pager).unwrap();
                eprintln!("dump slot {i}: left={} key={:?}", x.left_child, key);
            }
            BTreeCell::TableInterior(x) => {
                eprintln!("dump slot {i}: left={} rowid={}", x.left_child, x.rowid_boundary);
            }
            _ => eprintln!("dump slot {i}: leaf?!"),
        }
    }
}

fn dump_page_keys(db: &mut inkdb::db::Database<inkdb::vfs::disk::DiskVfs>, pn: u32) {
    use inkdb::storage::page::BTreePage;
    let ps = db.pager.page_size();
    let us = db.pager.usable_size();
    let guard = db.pager.get(pn).unwrap();
    let page = BTreePage::new(pn, ps, us, guard.bytes_as_ref()).unwrap();
    let n = page.no_of_cells().unwrap();
    eprintln!("dump page {pn}: kind={:?} cells={n} rmp={:?}", page.page_type().unwrap(), page.right_most_ptr().unwrap());
    for i in [0, n.saturating_sub(1)] {
        if let Ok(cell) = page.cell(i) {
            eprintln!("dump page {pn} edge cell {i}: {:?}", page.record_of(&cell, &mut db.pager).unwrap().first());
        }
    }
}

fn probe(n: u64, tag: &str) {
    let path = db_path(tag);
    build_users(&path, 4096, n);
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
    // Index root via the engine catalog (only one index exists).
    let root: u32 = {
        let master = inkdb::SqliteMaster::new(&mut db.pager).unwrap();
        master.indexes.values().next().unwrap().root_page
    };
    walk_refcounts(&mut db, root);
    commit_and_close(db);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn probe_5k() {
    probe(5_000, "probe5k");
}

#[test]
fn probe_20k() {
    probe(20_000, "probe20k");
}

#[test]
fn probe_60k() {
    probe(60_000, "probe60k");
}

#[test]
fn probe_100k_walk() {
    probe(100_000, "probe100k");
}

#[test]
fn probe_80k() {
    probe(80_000, "probe80k");
}

#[path = "common.rs"]
mod common;

use common::*;
use std::collections::{HashMap, HashSet};

/// Refcount walk over the TABLE tree (root 2): reports pages with >1 parent.
fn walk_table_refs(db: &mut inkdb::db::Database<inkdb::vfs::disk::DiskVfs>) -> Vec<(u32, Vec<(u32, u16)>)> {
    use inkdb::storage::cell::BTreeCell;
    use inkdb::storage::page::BTreePage;
    let ps = db.pager.page_size();
    let us = db.pager.usable_size();
    let mut counts: HashMap<u32, u32> = HashMap::new();
    let mut parents: HashMap<u32, Vec<(u32, u16)>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut stack = vec![2u32];
    while let Some(pn) = stack.pop() {
        if pn == 0 || !seen.insert(pn) {
            continue;
        }
        if seen.len() > 100000 {
            break;
        }
        let Ok(guard) = db.pager.get(pn) else { continue };
        let Ok(page) = BTreePage::new(pn, ps, us, guard.bytes()) else { continue };
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
    let mut multi: Vec<(u32, Vec<(u32, u16)>)> = counts
        .iter()
        .filter(|(_, c)| **c > 1)
        .map(|(p, _)| (*p, parents.get(p).cloned().unwrap_or_default()))
        .collect();
    multi.sort_unstable();
    multi
}

/// Same walk over ONE index tree root.
fn walk_index_refs(
    db: &mut inkdb::db::Database<inkdb::vfs::disk::DiskVfs>,
    root: u32,
) -> Vec<(u32, Vec<(u32, u16)>)> {
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
            break;
        }
        let Ok(guard) = db.pager.get(pn) else { continue };
        let Ok(page) = BTreePage::new(pn, ps, us, guard.bytes()) else { continue };
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
    let mut multi: Vec<(u32, Vec<(u32, u16)>)> = counts
        .iter()
        .filter(|(_, c)| **c > 1)
        .map(|(p, _)| (*p, parents.get(p).cloned().unwrap_or_default()))
        .collect();
    multi.sort_unstable();
    multi
}

#[test]
fn bisect_groups() {
    let path = db_path("bisect");
    build_users(&path, 512, 20_000);
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
    let master = inkdb::SqliteMaster::new(&mut db.pager).unwrap();
    let idx_root = master.indexes.values().next().unwrap().root_page;
    let check = |db: &mut inkdb::db::Database<inkdb::vfs::disk::DiskVfs>| {
        let mut out = walk_table_refs(db);
        out.extend(walk_index_refs(db, idx_root));
        out
    };
    let d0 = check(&mut db);
    assert!(d0.is_empty(), "dirty before any delete: {d0:?}");
    let deleted: Vec<u32> = (20..60).step_by(3).collect();
    for age in &deleted {
        run_ok(&mut db, &format!("delete from users where age = {age}"));
        let d = check(&mut db);
        if !d.is_empty() {
            panic!("first dirty right after deleting age={age}: {d:?}");
        }
    }
    commit_and_close(db);
    cleanup(&path);
}

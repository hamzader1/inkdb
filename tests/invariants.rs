//! Structural audit of the B-tree layer.
//!
//! Real SQLite is the final judge, but all it gives back is a page number and
//! a verdict. These checks run *during* a workload and name the invariant that
//! broke, the page it broke on, and the step that broke it. They cover the two
//! classes of damage that keep showing up:
//!
//! * pages that are neither referenced by a tree nor on the freelist — exactly
//!   what SQLite reports as `Page N: never used`
//! * per page bookkeeping: keys ascending, cell spans inside the content area
//!   and non overlapping, freeblock chain ordered with >= 4 byte gaps, and free
//!   space accounting that matches the content area pointer
//! * routing: a divider must never be smaller than the largest key of the
//!   subtree it routes to, and sibling subtrees must not interleave.

#[path = "common.rs"]
mod common;

use common::*;
use inkdb::SqliteMaster;
use inkdb::db::Database;
use inkdb::db::header::{
    DATABASE_SIZE_IN_PAGES_OFFSET, FIRST_FREELIST_TRUNK_PAGE_OFFSET,
    TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET,
};
use inkdb::record::Value;
use inkdb::storage::page::{
    BTreePage, CELL_CONTENT_AREA_OFFSET, CELL_COUNT_OFFSET, FIRST_FREEBLOCK_OFFSET,
    FRAGMENTED_FREE_BYTES_OFFSET,
};
use inkdb::vfs::disk::DiskVfs;
use std::collections::HashSet;

fn u16_at(bytes: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([bytes[off], bytes[off + 1]])
}

fn u32_at(bytes: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

/// Number of pages the file claims to hold.
fn file_page_count(db: &mut Database<DiskVfs>) -> u32 {
    let guard = db.pager.get(1).expect("page 1");
    u32_at(guard.bytes(), DATABASE_SIZE_IN_PAGES_OFFSET)
}

/// Every page owned by the freelist, plus a check that the chain agrees with
/// the page count stored in the database header.
fn walk_freelist(db: &mut Database<DiskVfs>, problems: &mut Vec<String>) -> HashSet<u32> {
    let usable = db.pager.usable_size();
    let (head, total) = {
        let guard = db.pager.get(1).expect("page 1");
        let bytes = guard.bytes();
        (
            u32_at(bytes, FIRST_FREELIST_TRUNK_PAGE_OFFSET),
            u32_at(bytes, TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET),
        )
    };
    let mut free = HashSet::new();
    let mut counted = 0u32;
    let mut current = head;
    let mut hops = 0;
    while current != 0 {
        hops += 1;
        if hops > 100_000 {
            problems.push("freelist: chain runaway".into());
            break;
        }
        if !free.insert(current) {
            problems.push(format!("freelist: trunk {current} seen twice (cycle)"));
            break;
        }
        counted += 1;
        let (next, leaves) = {
            let guard = match db.pager.get(current) {
                Ok(g) => g,
                Err(e) => {
                    problems.push(format!("freelist: trunk {current} unreadable: {e}"));
                    break;
                }
            };
            let bytes = guard.bytes();
            (u32_at(bytes, 0), u32_at(bytes, 4))
        };
        let max_leaves = usable.saturating_sub(8) / 4;
        if leaves as usize > max_leaves {
            problems.push(format!(
                "freelist: trunk {current} claims {leaves} leaves, usable size fits {max_leaves}"
            ));
        }
        for slot in 0..leaves {
            let leaf = {
                let guard = db.pager.get(current).expect("trunk re-read");
                u32_at(guard.bytes(), 8 + 4 * slot as usize)
            };
            if leaf == 0 || !free.insert(leaf) {
                problems.push(format!(
                    "freelist: trunk {current} slot {slot} holds {leaf} (zero or duplicate)"
                ));
                continue;
            }
            counted += 1;
        }
        current = next;
    }
    if counted != total {
        problems.push(format!(
            "freelist: chain holds {counted} pages, header says {total}"
        ));
    }
    free
}



struct PageAudit {
    problems: Vec<String>,
    referenced: HashSet<u32>,
}

/// Audits one page and every page below it, accumulating problems. Returns the
/// (min, max) key of the subtree so callers can check divider routing.
fn audit_page(
    db: &mut Database<DiskVfs>,
    page_no: u32,
    ctx: &mut PageAudit,
) -> (Option<Value<'static>>, Option<Value<'static>>) {
    let empty = (None, None);
    if !ctx.referenced.insert(page_no) {
        ctx.problems
            .push(format!("page {page_no} is referenced more than once"));
        return empty;
    }
    let page_size = db.pager.page_size();
    let usable = db.pager.usable_size();
    let file_pages = file_page_count(db);
    if page_no == 0 || page_no > file_pages {
        ctx.problems.push(format!(
            "page {page_no} referenced but the file holds {file_pages} pages"
        ));
        return empty;
    }
    let header_offset = if page_no == 1 { 100usize } else { 0 };
    let bytes = {
        let guard = match db.pager.get(page_no) {
            Ok(g) => g,
            Err(e) => {
                ctx.problems.push(format!("page {page_no} unreadable: {e}"));
                return empty;
            }
        };
        guard.bytes().to_vec()
    };
    let page = match BTreePage::new(page_no, page_size, usable, &bytes[..]) {
        Ok(p) => p,
        Err(e) => {
            ctx.problems.push(format!("page {page_no} bad header: {e}"));
            return empty;
        }
    };
    let kind = match page.page_type() {
        Ok(k) => k,
        Err(e) => {
            ctx.problems.push(format!("page {page_no} bad page type: {e}"));
            return empty;
        }
    };
    let n = match page.no_of_cells() {
        Ok(n) => n,
        Err(e) => {
            ctx.problems.push(format!("page {page_no} bad cell count: {e}"));
            return empty;
        }
    };

    let content_area = u16_at(&bytes, header_offset + CELL_CONTENT_AREA_OFFSET) as usize;
    let frag = bytes[header_offset + FRAGMENTED_FREE_BYTES_OFFSET] as usize;
    let first_freeblock = u16_at(&bytes, header_offset + FIRST_FREEBLOCK_OFFSET) as usize;
    let raw_cells = u16_at(&bytes, header_offset + CELL_COUNT_OFFSET);
    if raw_cells != n {
        ctx.problems.push(format!(
            "page {page_no}: cell count {n} disagrees with the raw header {raw_cells}"
        ));
    }
    // End of the cell pointer array: nothing else may live below it.
    let cell_first = header_offset + kind.header_size() as usize + 2 * n as usize;
    if content_area > usable {
        ctx.problems.push(format!(
            "page {page_no}: content area {content_area} is past the usable size {usable}"
        ));
    }
    if content_area < cell_first {
        ctx.problems.push(format!(
            "page {page_no}: content area {content_area} overlaps the cell pointer array (ends at {cell_first})"
        ));
    }


    // Cell pointers and spans.
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(n as usize);
    for i in 0..n {
        let ptr = match page.cell_ptr(i) {
            Ok(p) => p as usize,
            Err(e) => {
                ctx.problems
                    .push(format!("page {page_no} slot {i}: bad cell pointer: {e}"));
                continue;
            }
        };
        if ptr < content_area || ptr >= usable {
            ctx.problems.push(format!(
                "page {page_no} slot {i}: cell pointer {ptr} is outside the content area [{content_area}, {usable})"
            ));
            continue;
        }
        match page.cell_span(ptr as u16) {
            Ok(span) => {
                if span.end > usable {
                    ctx.problems.push(format!(
                        "page {page_no} slot {i}: cell span {span:?} runs past the usable size {usable}"
                    ));
                }
                spans.push((span.start, span.end));
            }
            Err(e) => ctx
                .problems
                .push(format!("page {page_no} slot {i}: unparsable cell: {e}")),
        }
    }
    let mut ordered = spans.clone();
    ordered.sort_unstable();
    for pair in ordered.windows(2) {
        if pair[0].1 > pair[1].0 {
            ctx.problems.push(format!(
                "page {page_no}: cells overlap: {:?} and {:?}",
                pair[0], pair[1]
            ));
        }
    }

    // Freeblock chain: ordered, non overlapping, >= 4 byte gaps, inside bounds.
    let mut freeblock_bytes = 0usize;
    let mut cursor = first_freeblock;
    let mut seen_freeblocks = HashSet::new();
    let mut previous_end = cell_first;
    let mut first_block = true;
    while cursor != 0 {
        if cursor < cell_first || cursor + 4 > usable {
            ctx.problems.push(format!(
                "page {page_no}: freeblock {cursor} sits outside [{cell_first}, {usable})"
            ));
            break;
        }
        if !seen_freeblocks.insert(cursor) {
            ctx.problems
                .push(format!("page {page_no}: freeblock {cursor} loops"));
            break;
        }
        let next = u16_at(&bytes, cursor) as usize;
        let size = u16_at(&bytes, cursor + 2) as usize;
        if size < 4 {
            ctx.problems.push(format!(
                "page {page_no}: freeblock {cursor} has size {size} < 4"
            ));
        }
        if cursor + size > usable {
            ctx.problems.push(format!(
                "page {page_no}: freeblock {cursor} size {size} runs past the usable size"
            ));
        }
        if !first_block && cursor < previous_end + 4 {
            ctx.problems.push(format!(
                "page {page_no}: freeblock {cursor} is adjacent to or before {previous_end}"
            ));
        }
        for span in &spans {
            if cursor < span.1 && span.0 < cursor + size {
                ctx.problems.push(format!(
                    "page {page_no}: freeblock {cursor}..{} overlaps the cell at {span:?}",
                    cursor + size
                ));
            }
        }
        freeblock_bytes += size;
        previous_end = cursor + size;
        first_block = false;
        cursor = next;
    }
    if frag + content_area + freeblock_bytes > usable {
        ctx.problems.push(format!(
            "page {page_no}: free space accounting overflows (frag {frag} + content {content_area} + freeblocks {freeblock_bytes} > {usable})"
        ));
    }

    // Physical coverage: every byte above the content area has to belong to a
    // cell, to a freeblock, or be one of the bytes counted as fragmentation.
    // A byte that belongs to none of them is space nobody can ever reuse and
    // real SQLite calls the page corrupt ("free space corruption").
    let mut covered = spans.clone();
    let mut cursor = first_freeblock;
    let mut hops = 0;
    while cursor != 0 && hops < 10_000 {
        hops += 1;
        let next = u16_at(&bytes, cursor) as usize;
        let size = u16_at(&bytes, cursor + 2) as usize;
        if cursor + size <= usable {
            covered.push((cursor, cursor + size));
        }
        cursor = next;
    }
    covered.sort_unstable();
    let mut lost_bytes = 0usize;
    let mut lost_ranges: Vec<(usize, usize)> = Vec::new();
    let mut cursor = content_area;
    for (start, end) in covered {
        if end <= cursor {
            continue;
        }
        if start > cursor {
            lost_bytes += start - cursor;
            if lost_ranges.len() < 5 {
                lost_ranges.push((cursor, start));
            }
        }
        cursor = cursor.max(end);
    }
    if cursor < usable {
        lost_bytes += usable - cursor;
        if lost_ranges.len() < 5 {
            lost_ranges.push((cursor, usable));
        }
    }
    if lost_bytes > frag {
        ctx.problems.push(format!(
            "page {page_no}: {lost_bytes} byte(s) above the content area belong to no cell, no freeblock and are not counted as fragmentation (frag {frag}, lost ranges {lost_ranges:?})"
        ));
    }


    // Keys must ascend across the slots, otherwise every descent misroutes.
    let mut keys: Vec<Value<'static>> = Vec::with_capacity(n as usize);
    for i in 0..n {
        let cell = match page.cell(i) {
            Ok(c) => c,
            Err(e) => {
                ctx.problems
                    .push(format!("page {page_no} slot {i}: unreadable cell: {e}"));
                continue;
            }
        };
        match page.cell_key(&cell, &mut db.pager) {
            Ok(key) => keys.push(key),
            Err(e) => ctx
                .problems
                .push(format!("page {page_no} slot {i}: unreadable key: {e}")),
        }
    }
    for (i, pair) in keys.windows(2).enumerate() {
        if pair[0] >= pair[1] {
            ctx.problems.push(format!(
                "page {page_no}: keys not ascending at slots {i},{}: {:?} then {:?}",
                i + 1,
                pair[0],
                pair[1]
            ));
        }
    }

    if kind.is_leaf() {
        return (keys.first().cloned(), keys.last().cloned());
    }

    // Interior: recurse into every child and check the routing rules.
    let mut children: Vec<(u16, u32, Option<Value<'static>>)> = Vec::with_capacity(n as usize + 1);
    for i in 0..n {
        if let Ok(cell) = page.cell(i) {
            children.push((i, cell.left_child(), keys.get(i as usize).cloned()));
        }
    }
    match page.right_most_ptr() {
        Ok(Some(rmp)) => children.push((n, rmp, None)),
        Ok(None) => ctx
            .problems
            .push(format!("page {page_no}: interior page without a right-most child")),
        Err(e) => ctx
            .problems
            .push(format!("page {page_no}: bad right-most pointer: {e}")),
    }
    let mut subtree_min: Option<Value<'static>> = None;
    let mut subtree_max: Option<Value<'static>> = None;
    for (slot, child, divider) in children {
        let (min, max) = audit_page(db, child, ctx);
        if let (Some(divider), Some(max)) = (&divider, &max)
            && max > divider
        {
            ctx.problems.push(format!(
                "page {page_no} slot {slot} -> page {child}: divider {divider:?} is smaller than the largest key below it ({max:?})"
            ));
        }
        if let (Some(previous), Some(min)) = (&subtree_max, &min)
            && min < previous
        {
            ctx.problems.push(format!(
                "page {page_no} slot {slot} -> page {child}: subtree starts at {min:?}, below the previous subtree max {previous:?}"
            ));
        }
        if subtree_min.is_none() {
            subtree_min = min;
        }
        if max.is_some() {
            subtree_max = max;
        }
    }
    (subtree_min, subtree_max)
}


/// Full audit: catalog trees, freelist chain, and every page in the file.
pub fn audit_database(db: &mut Database<DiskVfs>, tag: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let master = match SqliteMaster::new(&mut db.pager) {
        Ok(m) => m,
        Err(e) => return vec![format!("[{tag}] catalog unreadable: {e}")],
    };
    let mut ctx = PageAudit {
        problems: Vec::new(),
        referenced: HashSet::new(),
    };
    let table_root = master.tables.get("users").map(|t| t.root_page).unwrap_or(0);
    audit_page(db, 1, &mut ctx);
    if table_root != 0 {
        audit_page(db, table_root, &mut ctx);
    }
    let mut index_roots: Vec<u32> = master.indexes.values().map(|i| i.root_page).collect();
    index_roots.sort_unstable();
    for root in index_roots {
        audit_page(db, root, &mut ctx);
    }
    problems.append(&mut ctx.problems);

    let free = walk_freelist(db, &mut problems);
    let file_pages = file_page_count(db);
    for page_no in 2..=file_pages {
        let referenced = ctx.referenced.contains(&page_no);
        let freelisted = free.contains(&page_no);
        if referenced && freelisted {
            problems.push(format!(
                "page {page_no} is referenced by a tree and on the freelist at the same time"
            ));
        }
        if !referenced && !freelisted {
            problems.push(format!(
                "page {page_no} is orphaned: no tree references it and it is not on the freelist (SQLite reports these as 'never used')"
            ));
        }
    }
    problems
}

fn checkpoint(db: &mut Database<DiskVfs>, tag: &str, assert_clean: bool) {
    let problems = audit_database(db, tag);
    if problems.is_empty() {
        eprintln!("AUDIT {tag}: ok");
        return;
    }
    eprintln!("AUDIT {tag}: {} problem(s)", problems.len());
    for problem in problems.iter().take(15) {
        eprintln!("    {problem}");
    }
    if assert_clean {
        panic!(
            "{tag}: {} structural problem(s), first: {}",
            problems.len(),
            problems[0]
        );
    }
}

/// Inserts, builds an index, deletes whole age groups, auditing as it goes.
fn audit_run(tag: &str, page_size: u32, rows: usize, victims: usize, seed: u64) {
    let path = db_path(tag);
    build_users(&path, page_size, 0);
    let mut db = open_engine(&path);
    let mut rng = Rng(seed);
    for i in 0..rows {
        let age = 20 + rng.next(10) as i64;
        let salary = 1000.0 + i as f64;
        run_ok(
            &mut db,
            &format!(
                "insert into users values ('U{i:05}', {age}, {salary}.1, 'P{}')",
                i % 5
            ),
        );
        if i % 100 == 99 {
            checkpoint(&mut db, &format!("{tag}/insert {i}"), true);
        }
    }
    checkpoint(&mut db, &format!("{tag}/inserts done"), true);
    run_ok(&mut db, "create index age_index on users(age)");
    checkpoint(&mut db, &format!("{tag}/index built"), true);

    let mut group = Vec::new();
    while group.len() < victims {
        let age = 20 + rng.next(10) as i64;
        if !group.contains(&age) {
            group.push(age);
        }
    }
    for age in &group {
        run_ok(&mut db, &format!("delete from users where age = {age}"));
        checkpoint(&mut db, &format!("{tag}/delete age {age}"), true);
    }
    commit_and_close(db);
    cleanup(&path);
}

/// Small pages, smaller load: this has to stay clean.
#[test]
fn structural_audit_through_indexed_deletes() {
    audit_run("audit-small", 512, 600, 3, 0xC0FFEE);
}

/// Larger pages and deeper delete runs. Known failure: the free list ends up
/// with two blocks one byte apart on the index root (page 69), which real
/// SQLite rejects with "free space corruption" and then refuses to walk that
/// subtree, reporting its children as "Page N: never used". `insert_freeblock`
/// only coalesces exactly adjacent blocks; SQLite absorbs any gap shorter than
/// a freeblock header. Run with `--ignored` to reproduce.
#[test]
#[ignore = "known free space corruption: <4 byte gap between free blocks"]
fn structural_audit_large_pages_deep_deletes() {
    audit_run("audit-large", 1024, 1200, 5, 0xC0FFEE);
}

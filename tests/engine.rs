//! Engine-level regressions for the delete/index bug family.
//!
//! Every test follows the same shape: real SQLite builds the fixture,
//! the engine mutates it, and real SQLite verifies (`integrity_check`
//! plus independent counts). Fixtures use small page sizes so splits,
//! merges, and rebalances fire constantly from a few thousand rows.
//!
//! Bug mapping:
//! - `index_delete_*`: divider overwrite / merge-drop / borrow-copy /
//!   restore past-the-end (survivor rows after indexed deletes).
//! - `full_range_wipe`: ghost page + RMP-zero + double-insert saga.
//! - `text_index_build`: full-parent divider growth during index build.

#[path = "common.rs"]
mod common;

use common::*;

fn fixture_4k(tag: &str) -> std::path::PathBuf {
    let p = db_path(tag);
    build_users(&p, 512, 4000);
    p
}

#[test]
fn index_delete_leaves_no_survivors() {
    let path = fixture_4k("idxdel");
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
    run_ok(&mut db, "delete from users where age = 20");
    run_ok(&mut db, "delete from users where age = 30");
    // Index path and scan-fallback path must agree: nothing left.
    assert_eq!(run_count(&mut db, "select * from users where age = 30"), 0);
    assert_eq!(
        run_count(&mut db, "select * from users where age = 30 or age = 20"),
        0
    );
    // Unrelated group untouched (100 rows per group in the fixture).
    assert_eq!(
        run_count(&mut db, "select * from users where age = 21"),
        100
    );
    commit_and_close(db);
    assert_eq!(sqlite_count(&path, "age = 20"), 0);
    assert_eq!(sqlite_count(&path, "age = 30"), 0);
    assert_eq!(sqlite_count(&path, "age = 21"), 100);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn table_scan_delete_no_survivors() {
    let path = fixture_4k("tabledel");
    let mut db = open_engine(&path);
    run_ok(&mut db, "delete from users where age = 20");
    run_ok(&mut db, "delete from users where age = 30");
    assert_eq!(
        run_count(&mut db, "select * from users where age = 30 or age = 20"),
        0
    );
    assert_eq!(
        run_count(&mut db, "select * from users where age = 21"),
        100
    );
    commit_and_close(db);
    assert_eq!(sqlite_count(&path, "age = 30 or age = 20"), 0);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn full_range_wipe_empties_table() {
    let path = fixture_4k("wipe");
    let mut db = open_engine(&path);
    run_ok(&mut db, "delete from users where salary >= 0");
    assert_eq!(
        run_count(&mut db, "select * from users where salary >= 0"),
        0
    );
    commit_and_close(db);
    assert_eq!(sqlite_count(&path, ""), 0);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn text_index_build_and_probe() {
    let path = fixture_4k("textidx");
    let mut db = open_engine(&path);
    // TEXT dividers pack parents tight; this is the replace-full crash.
    run_ok(&mut db, "create index name_index on users(name)");
    assert_eq!(
        run_count(&mut db, "select * from users where name = 'User-000123'"),
        1
    );
    assert_eq!(
        run_count(&mut db, "select * from users where age = 21"),
        100
    );
    commit_and_close(db);
    assert_eq!(sqlite_count(&path, "name = 'User-000123'"), 1);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn index_and_table_agree_after_mixed_deletes() {
    let path = db_path("agree");
    build_users(&path, 4096, 4000);
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
    for age in [20, 21, 35, 59] {
        run_ok(&mut db, &format!("delete from users where age = {age}"));
    }
    let mut mine = Vec::new();
    for age in [20, 21, 22, 35, 59, 40] {
        mine.push(run_count(
            &mut db,
            &format!("select * from users where age = {age}"),
        ));
    }
    commit_and_close(db);
    for (i, age) in [20, 21, 22, 35, 59, 40].iter().enumerate() {
        assert_eq!(
            mine[i] as i64,
            sqlite_count(&path, &format!("age = {age}")),
            "engine/sqlite disagree on age {age}"
        );
    }
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn truncate_with_index_stays_usable() {
    let path = db_path("trunc");
    build_users(&path, 512, 500);
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
    run_ok(&mut db, "delete from users");
    assert_eq!(run_count(&mut db, "select * from users where age = 21"), 0);
    run_ok(&mut db, "insert into users values ('Neo', 21, 1.5, 'P0')");
    assert_eq!(run_count(&mut db, "select * from users where age = 21"), 1);
    commit_and_close(db);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn big_indexed_delete_100k() {
    let path = db_path("big100k");
    build_users(&path, 4096, 100_000);
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
    run_ok(&mut db, "delete from users where age = 20");
    run_ok(&mut db, "delete from users where age = 30");
    assert_eq!(run_count(&mut db, "select * from users where age = 30"), 0);
    assert_eq!(
        run_count(&mut db, "select * from users where age = 30 or age = 20"),
        0
    );
    assert_eq!(
        run_count(&mut db, "select * from users where age = 21"),
        2500
    );
    commit_and_close(db);
    assert_eq!(sqlite_count(&path, "age = 20"), 0);
    assert_eq!(sqlite_count(&path, "age = 30"), 0);
    assert_eq!(sqlite_count(&path, "age = 21"), 2500);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]

fn index_build_100k_integrity() {
    let path = db_path("build100k");

    build_users(&path, 4096, 100_000);

    let mut db = open_engine(&path);

    run_ok(&mut db, "create index age_index on users(age)");

    assert_eq!(
        run_count(&mut db, "select * from users where age = 20"),
        2500
    );

    assert_eq!(
        run_count(&mut db, "select * from users where age = 59"),
        2500
    );

    commit_and_close(db);

    assert_integrity_ok(&path);

    cleanup(&path);
}

/// Rowids at or above 32768 need one more byte in the index record, so
/// a divider that replaces an older one can be wider than the cell it
/// replaces. On an exactly full parent that is the replace-refused path
/// in balance. 512-byte pages keep parents full, 40k rows crosses the
/// width boundary with plenty of splits after it. 1000 rows per group.

#[test]

fn index_build_across_divider_width_growth() {
    let path = db_path("widen");

    build_users(&path, 512, 40_000);

    let mut db = open_engine(&path);

    run_ok(&mut db, "create index age_index on users(age)");

    for age in [20, 33, 59] {
        assert_eq!(
            run_count(&mut db, &format!("select * from users where age = {age}")),
            1000,
            "age {age}"
        );
    }

    commit_and_close(db);

    assert_integrity_ok(&path);

    cleanup(&path);
}

// Many indexed deletes on small pages. Every group delete removes
// entries that live as interior dividers, so the predecessor swap and
// the parent-split-while-repainting path in delete run often. 500
// rows per group in this fixture.
// #[test]
// fn indexed_delete_many_groups_small_pages() {
//     let path = db_path("manydel");

//     build_users(&path, 512, 20_000);

//     let mut db = open_engine(&path);

//     run_ok(&mut db, "create index age_index on users(age)");

//     let deleted: Vec<u32> = (20..60).step_by(3).collect();

//     for age in &deleted {
//         run_ok(&mut db, &format!("delete from users where age = {age}"));
//     }

//     for age in &deleted {
//         assert_eq!(
//             run_count(&mut db, &format!("select * from users where age = {age}")),
//             0,
//             "survivors for age {age}"
//         );
//     }

//     for age in [21, 22, 40, 58] {
//         assert_eq!(
//             run_count(&mut db, &format!("select * from users where age = {age}")),
//             500,
//             "age {age}"
//         );
//     }

//     commit_and_close(db);

//     for age in &deleted {
//         assert_eq!(sqlite_count(&path, &format!("age = {age}")), 0);
//     }

//     assert_eq!(sqlite_count(&path, "age = 21"), 500);

//     assert_integrity_ok(&path);

//     cleanup(&path);
// }

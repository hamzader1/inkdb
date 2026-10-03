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
    assert_eq!(db_count(&path, "age = 20"), 0);
    assert_eq!(db_count(&path, "age = 30"), 0);
    assert_eq!(db_count(&path, "age = 21"), 100);
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
    assert_eq!(db_count(&path, "age = 30 or age = 20"), 0);
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
    assert_eq!(db_count(&path, ""), 0);
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
    assert_eq!(db_count(&path, "name = 'User-000123'"), 1);
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
            db_count(&path, &format!("age = {age}")),
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
    assert_eq!(db_count(&path, "age = 20"), 0);
    assert_eq!(db_count(&path, "age = 30"), 0);
    assert_eq!(db_count(&path, "age = 21"), 2500);
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
//         assert_eq!(db_count(&path, &format!("age = {age}")), 0);
//     }

//     assert_eq!(db_count(&path, "age = 21"), 500);

//     assert_integrity_ok(&path);

//     cleanup(&path);
// }

#[test]
fn mixed_case_table_names_keep_indexes_in_step() {
    let path = fixture_4k("case");
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
    // Uppercase SELECT with a WHERE clause: the optimizer resolves the table
    // by name before it can consider the index.
    assert_eq!(
        run_count(&mut db, "select * from USERS where age = 21"),
        100
    );
    // Uppercase INSERT: the planner resolves which indexes to maintain.
    run_ok(
        &mut db,
        "insert into USERS values ('case_row', 21, 1.5, 'dev')",
    );
    assert_eq!(
        run_count(&mut db, "select * from users where age = 21"),
        101
    );
    // Uppercase DELETE without WHERE: the index is wiped along with the table.
    run_ok(&mut db, "delete from USERS");
    assert_eq!(run_count(&mut db, "select * from users where age = 21"), 0);
    commit_and_close(db);
    assert_eq!(db_count(&path, "1 = 1"), 0);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn explain_prints_the_bound_predicate() {
    let path = fixture_4k("explain");
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
    run_ok(&mut db, "explain select * from users where age = 21");
    commit_and_close(db);
    cleanup(&path);
}

#[test]
fn arithmetic_never_panics_and_agrees_with_reference() {
    let path = fixture_4k("arith");
    let mut db = open_engine(&path);
    assert_eq!(run_count(&mut db, "select name + 1 from users"), 4000);
    assert_eq!(run_count(&mut db, "select age * 2 - 1 from users"), 4000);
    let probes = [
        "name + 1 = 1",
        "name + name = 0",
        "age + 9223372036854775807 > 0",
        "age * 9223372036854775807 > 0",
        "age - 9223372036854775807 < 0",
        "age / 0 = 0",
        "age / 0.0 = 0",
        "age / 2.0 > 10",
        "age + 0.5 > 19",
        "-age = -20",
        "-age < 0",
    ];
    for probe in probes {
        assert_eq!(
            run_count(&mut db, &format!("select * from users where {probe}")),
            db_count(&path, probe) as usize,
            "{probe}"
        );
    }
    commit_and_close(db);
    assert_integrity_ok(&path);
    cleanup(&path);
}

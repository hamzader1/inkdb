//! Randomized mixed workload checked against an in-memory model.
//!
//! Inserts go through the engine's own INSERT path, deletes through the
//! indexed delete path, and every step is verified against a HashMap
//! model plus real SQLite at the end. Deterministic seed: same ops,
//! same order, every run.

#[path = "common.rs"]
mod common;

use common::*;
use std::collections::HashMap;

// FAILED:
// thread 'random_inserts_then_indexed_deletes_match_model' (3836043) panicked at tests/common.rs:111:23:
/*
// exec "select * from users where age = 20": index 174 holds rowid 596 but table 2 has no such row
// stack backtrace:
//    0: __rustc::rust_begin_unwind
//    1: core::panicking::panic_fmt
//    2: workload::common::run_count
//    3: <workload::random_inserts_then_indexed_deletes_match_model::{closure#0} as core::ops::function::FnOnce<()>>::call_once
// note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
*/
#[test]
fn random_inserts_then_indexed_deletes_match_model() {
    let path = db_path("workload");
    build_users(&path, 512, 0);
    let mut db = open_engine(&path);

    // 1500 engine inserts, ages confined to 10 groups so deletes hit hard.
    let mut rng = Rng(0xC0FFEE);
    let mut model: HashMap<i64, i64> = HashMap::new();
    for i in 0..1500 {
        let age = 20 + rng.next(10) as i64;
        let salary = 1000.0 + i as f64;
        run_ok(
            &mut db,
            &format!(
                "insert into users values ('U{i:05}', {age}, {salary}.1, 'P{}')",
                i % 5
            ),
        );
        *model.entry(age).or_insert(0) += 1;
    }
    for age in 20..30 {
        let mine = run_count(&mut db, &format!("select * from users where age = {age}"));
        assert_eq!(
            mine as i64,
            model.get(&age).copied().unwrap_or(0),
            "insert model drift on age {age}"
        );
    }

    // Index everything, then delete 4 random groups through the index.
    run_ok(&mut db, "create index age_index on users(age)");
    let mut victims = Vec::new();
    while victims.len() < 4 {
        let age = 20 + rng.next(10) as i64;
        if !victims.contains(&age) {
            victims.push(age);
        }
    }
    let mut deleted = 0i64;
    for age in &victims {
        run_ok(&mut db, &format!("delete from users where age = {age}"));
        deleted += model.remove(age).unwrap_or(0);
    }
    assert_eq!(1500 - deleted, model.values().sum::<i64>());

    // Exact agreement per group plus totals, engine side first.
    let mut total = 0usize;
    for age in 20..30 {
        let mine = run_count(&mut db, &format!("select * from users where age = {age}"));
        dbg!(mine);
        let want = model.get(&age).copied().unwrap_or(0);
        dbg!(want);
        assert_eq!(mine as i64, want, "post-delete drift on age {age}");
        total += mine;
    }
    assert_eq!(total as i64, 1500 - deleted);

    commit_and_close(db);
    // Independent SQLite recount of every group.
    let conn = rusqlite::Connection::open(&path).unwrap();
    for age in 20..30 {
        let n: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM users WHERE age = {age}"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n,
            model.get(&age).copied().unwrap_or(0),
            "sqlite drift age {age}"
        );
    }
    drop(conn);
    assert_integrity_ok(&path);
    cleanup(&path);
}

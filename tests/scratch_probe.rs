#[path = "common.rs"]
mod common;

use common::*;

fn probe(n: u64, tag: &str) {
    let path = db_path(tag);
    build_users(&path, 4096, n);
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
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

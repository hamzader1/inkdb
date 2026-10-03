#[path = "common.rs"]
mod common;

use common::*;
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use std::time::{Duration, Instant};

fn drain(db: &mut Database<DiskVfs>, q: &str) -> usize {
    let mut stmt = match db.execute(q) {
        Ok(s) => s,
        Err(e) => panic!("exec {q}: {e}"),
    };
    let mut n = 0;
    for row in stmt.rows() {
        row.expect("row");
        n += 1;
    }
    n
}

fn bench(db: &mut Database<DiskVfs>, label: &str, q: &str, reps: u32) {
    let mut best = Duration::MAX;
    let mut rows = 0;
    for _ in 0..reps {
        let start = Instant::now();
        rows = drain(db, q);
        best = best.min(start.elapsed());
    }
    let per = best.as_secs_f64();
    println!(
        "{label:<34} rows={rows:<7} {:.3} ms/run   {:.0} ns/row",
        per * 1000.0,
        per * 1e9 / rows.max(1) as f64
    );
}

#[test]
fn timings() {
    let path = db_path("bench");
    build_users(&path, 4096, 100_000);
    let mut db = open_engine(&path);
    run_ok(&mut db, "create index age_index on users(age)");
    run_ok(&mut db, "create index name_index on users(name)");
    commit_and_close(db);

    let mut db = open_engine(&path);
    bench(&mut db, "select * from users", "select * from users", 7);
    bench(
        &mut db,
        "select name from users",
        "select name from users",
        7,
    );
    bench(
        &mut db,
        "select name, age, salary",
        "select name, age, salary from users",
        7,
    );
    bench(&mut db, "select count(*)", "select count(*) from users", 7);
    bench(
        &mut db,
        "where age = 21 (index)",
        "select * from users where age = 21",
        5,
    );
    bench(
        &mut db,
        "where age = 21 (count)",
        "select count(*) from users where age = 21",
        5,
    );
    bench(
        &mut db,
        "where name = 'User-099999'",
        "select * from users where name = 'User-099999'",
        5,
    );
    bench(
        &mut db,
        "explain select",
        "explain select * from users where age = 21",
        200,
    );

    let start = Instant::now();
    let mut inserted = 0;
    for i in 0..2_000 {
        let mut stmt = db
            .execute(&format!(
                "insert into users values ('New-{i}', {i}, {i}.5, 'P-{i}')"
            ))
            .expect("insert");
        for row in stmt.rows() {
            row.expect("row");
        }
        inserted += 1;
    }
    let per = start.elapsed().as_secs_f64() / f64::from(inserted);
    println!(
        "{:<34} rows={inserted:<7} {:.3} ms/run   {:.0} ns/row",
        "insert into users",
        per * 1000.0,
        per * 1e9
    );
    commit_and_close(db);
    cleanup(&path);
}

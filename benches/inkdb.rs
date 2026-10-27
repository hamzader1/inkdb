use criterion::{Criterion, criterion_group, criterion_main};
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use std::path::PathBuf;
use std::time::Duration;

fn fixture() -> PathBuf {
    let p = std::env::temp_dir().join("inkdb-crit.db");
    let _ = std::fs::remove_file(&p);
    let conn = rusqlite::Connection::open(&p).unwrap();
    conn.execute_batch("PRAGMA page_size=4096; PRAGMA journal_mode=DELETE;")
        .unwrap();
    conn.execute_batch("VACUUM;").unwrap();
    conn.execute_batch(
        "CREATE TABLE users (name VARCHAR(100), age INT, salary DOUBLE, position VARCHAR(100));",
    )
    .unwrap();
    conn.execute_batch("BEGIN;").unwrap();
    {
        let mut stmt = conn.prepare("INSERT INTO users VALUES ('User-' || printf('%06d', ?1), 20 + (?1 % 40), 30000.0 + ((?1 * 13) % 60000), 'Position-' || (?1 % 50))").unwrap();
        for x in 1..=100_000i64 {
            stmt.execute([x]).unwrap();
        }
    }
    conn.execute_batch("COMMIT;").unwrap();
    drop(conn);
    p
}

fn open() -> Database<DiskVfs> {
    let p = fixture();
    let mut db = Database::new(&p).unwrap();
    // create indexes once
    let mut s = db.execute("create index age_index on users(age)").unwrap();
    for _ in s.rows() {}
    let mut s = db
        .execute("create index name_index on users(name)")
        .unwrap();
    for _ in s.rows() {}
    db.pager().commit().unwrap();
    db
}

fn bench_queries(c: &mut Criterion) {
    let mut db = open();
    let mut group = c.benchmark_group("inkdb");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("select_star", |b| {
        b.iter(|| {
            let mut s = db.execute("select * from users").unwrap();
            let mut n = 0usize;
            for r in s.rows() {
                r.unwrap();
                n += 1;
            }
            n
        })
    });

    group.bench_function("count_star", |b| {
        b.iter(|| {
            let mut s = db.execute("select count(*) from users").unwrap();
            let mut n = 0usize;
            for r in s.rows() {
                r.unwrap();
                n += 1;
            }
            n
        })
    });

    group.bench_function("index_age_eq", |b| {
        b.iter(|| {
            let mut s = db.execute("select * from users where age = 21").unwrap();
            let mut n = 0usize;
            for r in s.rows() {
                r.unwrap();
                n += 1;
            }
            n
        })
    });

    group.bench_function("index_name_lookup", |b| {
        b.iter(|| {
            let mut s = db
                .execute("select * from users where name = 'User-099999'")
                .unwrap();
            let mut n = 0usize;
            for r in s.rows() {
                r.unwrap();
                n += 1;
            }
            n
        })
    });

    group.bench_function("order_by_age_limit_10", |b| {
        b.iter(|| {
            let mut s = db
                .execute("select * from users order by age asc limit 10")
                .unwrap();
            let mut n = 0usize;
            for r in s.rows() {
                r.unwrap();
                n += 1;
            }
            n
        })
    });

    group.finish();
    // keep db alive
    let _ = &mut db;
}

criterion_group!(benches, bench_queries);
criterion_main!(benches);

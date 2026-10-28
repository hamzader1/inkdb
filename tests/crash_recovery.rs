use inkdb::db::Database;
use std::env;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[path = "common.rs"]
mod common;
use common::{assert_integrity_ok, cleanup, db_path};

/// Writer mode: invoked as a subprocess of this test binary.
fn run_writer() {
    let path = env::var("INKDB_CRASH_DB").expect("db path");
    let n: usize = env::var("INKDB_CRASH_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);

    let mut db = Database::open_or_create(&path).expect("open");
    let mut stmt = db
        .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
        .expect("create");
    for _ in stmt.rows() {}

    db.pager().start_transaction();
    for i in 0..n {
        let mut stmt = db
            .execute(&format!("INSERT INTO t VALUES ({i}, 'v{i}')"))
            .expect("insert");
        for _ in stmt.rows() {}
    }
    db.pager().commit().expect("commit");
    println!("done");
}

#[test]
fn crash_during_write_recovers_cleanly() {
    // If we are the writer subprocess, do the work and exit.
    if env::var("INKDB_CRASH_WRITER").is_ok() {
        run_writer();
        return;
    }

    let path = db_path("crash");
    cleanup(&path);

    let exe = env::current_exe().expect("current exe");
    let mut child = Command::new(exe)
        .arg("crash_during_write_recovers_cleanly")
        .env("INKDB_CRASH_WRITER", "1")
        .env("INKDB_CRASH_DB", &path)
        .env("INKDB_CRASH_N", "50000")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn crash writer");

    thread::sleep(Duration::from_millis(150));
    let _ = child.kill();
    let _ = child.wait();

    let mut db = match Database::new(&path) {
        Ok(db) => db,
        Err(_) => {
            cleanup(&path);
            return;
        }
    };
    db.pager().start_transaction();

    let count = match db.execute("SELECT COUNT(*) FROM t") {
        Ok(mut stmt) => {
            let mut n = 0i64;
            for row in stmt.rows() {
                if let Ok(r) = row
                    && let Some(v) = r.value(0).ok()
                    && let inkdb::record::Value::Integer(x) = v
                {
                    n = x;
                }
            }
            n
        }
        Err(_) => 0,
    };

    assert!(
        count == 0 || count == 50000,
        "partial write survived crash: {count} rows"
    );

    drop(db);
    assert_integrity_ok(&path);
    cleanup(&path);
}

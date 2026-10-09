use inkdb::backend::executor::RowWrapper;
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "constraints.db".to_owned());
    let mut db = Database::open_or_create(path)?;

    if db.master().table("customers").is_none() {
        execute(
            &mut db,
            "CREATE TABLE customers (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                email TEXT UNIQUE,
                age INTEGER,
                city TEXT,
                is_active BOOL DEFAULT TRUE
            )",
        )?;
        execute(
            &mut db,
            "INSERT INTO customers VALUES
                (1, 'Alice Johnson', 'alice@example.com', 28, 'Casablanca', TRUE)",
        )?;
    }

    expect_failure(
        &mut db,
        "INSERT INTO customers VALUES
            (2, 'Another Alice', 'alice@example.com', 30, 'Rabat', TRUE)",
    )?;
    expect_failure(
        &mut db,
        "INSERT INTO customers VALUES
            (1, 'Duplicate ID', 'duplicate@example.com', 30, 'Fes', TRUE)",
    )?;
    expect_failure(
        &mut db,
        "INSERT INTO customers VALUES
            (3, NULL, 'nullname@example.com', 25, 'Fes', TRUE)",
    )?;

    println!("The original row remains unchanged:");
    execute(
        &mut db,
        "SELECT id, name, email FROM customers WHERE id = 1",
    )?;
    Ok(())
}

fn expect_failure(db: &mut Database<DiskVfs>, sql: &str) -> Result<(), Box<dyn Error>> {
    match execute(db, sql) {
        Ok(()) => {
            Err(std::io::Error::other(format!("expected this statement to fail: {sql}")).into())
        }
        Err(error) => {
            println!("Rejected as expected: {error}");
            Ok(())
        }
    }
}

fn execute(db: &mut Database<DiskVfs>, sql: &str) -> Result<(), Box<dyn Error>> {
    let mut statement = db.execute(sql)?;
    for row in statement.rows() {
        println!("{}", RowWrapper(row?));
    }
    Ok(())
}

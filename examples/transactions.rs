use inkdb::backend::executor::RowWrapper;
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "transactions.db".to_owned());
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
                (1, 'Alice Johnson', 'alice@example.com', 28, 'NewYork', TRUE)",
        )?;
    }

    println!("COMMIT TEST:");
    execute(&mut db, "BEGIN")?;
    println!("Before commit");
    execute(&mut db, "SELECT id, name, city FROM customers WHERE id = 1")?;
    execute(&mut db, "UPDATE customers SET city = 'Paris' WHERE id = 1")?;
    execute(&mut db, "COMMIT")?;
    println!("After commit");
    execute(&mut db, "SELECT id, name, city FROM customers WHERE id = 1")?;

    println!("ROLLACK TEST:");
    execute(&mut db, "BEGIN")?;
    execute(&mut db, "UPDATE customers SET city = 'Madrid' WHERE id = 1")?;
    println!("Before calling rollback");
    execute(&mut db, "SELECT id, name, city FROM customers WHERE id = 1")?;
    execute(&mut db, "ROLLBACK")?;
    println!("After calling rollback");
    execute(&mut db, "SELECT id, name, city FROM customers WHERE id = 1")?;
    Ok(())
}

fn execute(db: &mut Database<DiskVfs>, sql: &str) -> Result<(), Box<dyn Error>> {
    let mut statement = db.execute(sql)?;
    for row in statement.rows() {
        println!("{}", RowWrapper(row?));
    }
    Ok(())
}

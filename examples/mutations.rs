use inkdb::backend::executor::RowWrapper;
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mutations.db".to_owned());
    let mut db = Database::open_or_create(path)?;

    if db.master().table("products").is_none() {
        execute(
            &mut db,
            "CREATE TABLE products (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                category TEXT NOT NULL,
                price FLOAT NOT NULL,
                stock INTEGER DEFAULT 0,
                sku TEXT UNIQUE
            )",
        )?;
        execute(
            &mut db,
            "INSERT INTO products VALUES
                (1, 'Laptop Pro', 'Electronics', 1299.99, 15, 'ELEC-001'),
                (2, 'Wireless Mouse', 'Electronics', 24.99, 100, 'ELEC-002'),
                (3, 'Notebook', 'Stationery', 3.50, 200, 'STAT-001')",
        )?;
        execute(
            &mut db,
            "CREATE INDEX idx_products_category ON products (category)",
        )?;
    }

    println!("Update an indexed value:");
    execute(
        &mut db,
        "UPDATE products SET category = 'Office' WHERE id = 1",
    )?;
    execute(
        &mut db,
        "SELECT id, name, category FROM products WHERE category = 'Office'",
    )?;
    execute(
        &mut db,
        "SELECT id, name FROM products WHERE category = 'Electronics'",
    )?;

    println!("Update stock with an arithmetic expression:");
    execute(
        &mut db,
        "UPDATE products SET stock = stock + 1 WHERE id = 2",
    )?;
    execute(&mut db, "SELECT id, name, stock FROM products WHERE id = 2")?;

    println!("Delete a group of rows:");
    execute(
        &mut db,
        "DELETE FROM products WHERE category = 'Stationery'",
    )?;
    execute(
        &mut db,
        "SELECT id, name FROM products WHERE category = 'Stationery'",
    )?;
    Ok(())
}

fn execute(db: &mut Database<DiskVfs>, sql: &str) -> Result<(), Box<dyn Error>> {
    let mut statement = db.execute(sql)?;
    for row in statement.rows() {
        println!("{}", RowWrapper(row?));
    }
    Ok(())
}

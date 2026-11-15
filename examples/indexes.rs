use inkdb::backend::executor::RowWrapper;
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "indexes.db".to_owned());
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
                (3, 'Office Chair', 'Furniture', 249.50, 20, 'FURN-001'),
                (4, 'Notebook', 'Stationery', 3.50, 200, 'STAT-001')",
        )?;
    }

    for (name, column) in [
        ("idx_products_category", "category"),
        ("idx_products_price", "price"),
    ] {
        if db.master().index(name).is_none() {
            execute(
                &mut db,
                &format!("CREATE INDEX {name} ON products ({column})"),
            )?;
        }
    }

    println!("Plan for an indexed equality lookup:");
    execute(
        &mut db,
        "EXPLAIN SELECT * FROM products WHERE category = 'Electronics'",
    )?;

    println!("Plan for an indexed range lookup:");
    execute(
        &mut db,
        "EXPLAIN SELECT * FROM products WHERE price >= 50 AND price <= 500",
    )?;

    println!("Matching products:");
    execute(
        &mut db,
        "SELECT name, price FROM products
         WHERE price >= 50 AND price <= 500
         ORDER BY price ASC",
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

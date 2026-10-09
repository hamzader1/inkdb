use inkdb::backend::executor::RowWrapper;
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "store.db".to_owned());
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
            "CREATE TABLE orders (
                id INTEGER PRIMARY KEY,
                customer_id INTEGER NOT NULL,
                product_id INTEGER NOT NULL,
                quantity INTEGER NOT NULL,
                status TEXT DEFAULT 'pending',
                total FLOAT NOT NULL
            )",
        )?;

        execute(
            &mut db,
            "INSERT INTO customers VALUES
                (1, 'Alice Johnson', 'alice@example.com', 28, 'Casablanca', TRUE),
                (2, 'Bob Smith', 'bob@example.com', 35, 'Rabat', TRUE),
                (3, 'Charlie Brown', 'charlie@example.com', 22, 'Fes', TRUE)",
        )?;
        execute(
            &mut db,
            "INSERT INTO products VALUES
                (1, 'Laptop Pro', 'Electronics', 1299.99, 15, 'ELEC-001'),
                (2, 'Wireless Mouse', 'Electronics', 24.99, 100, 'ELEC-002'),
                (3, 'Office Chair', 'Furniture', 249.50, 20, 'FURN-001')",
        )?;
        execute(
            &mut db,
            "INSERT INTO orders VALUES
                (1, 1, 1, 1, 'completed', 1299.99),
                (2, 2, 2, 2, 'completed', 49.98),
                (3, 3, 3, 1, 'pending', 249.50)",
        )?;
    }

    if db.master().index("idx_customers_city").is_none() {
        execute(
            &mut db,
            "CREATE INDEX idx_customers_city ON customers (city)",
        )?;
    }
    if db.master().index("idx_products_category").is_none() {
        execute(
            &mut db,
            "CREATE INDEX idx_products_category ON products (category)",
        )?;
    }
    if db.master().index("idx_orders_status").is_none() {
        execute(&mut db, "CREATE INDEX idx_orders_status ON orders (status)")?;
    }

    println!("Customers in the requested age range:");
    execute(
        &mut db,
        "SELECT id, name, city FROM customers
         WHERE age >= 25 AND age <= 35
         ORDER BY age ASC
         LIMIT 5",
    )?;

    println!("Electronics ordered by price:");
    execute(
        &mut db,
        "SELECT name, price FROM products
         WHERE category = 'Electronics'
         ORDER BY price DESC
         LIMIT 3",
    )?;

    println!("Pending orders:");
    execute(
        &mut db,
        "SELECT id, status, total FROM orders
         WHERE status = 'pending'
         ORDER BY total DESC",
    )?;

    println!("Query plan for a city lookup:");
    execute(
        &mut db,
        "EXPLAIN SELECT * FROM customers WHERE city = 'Fes'",
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

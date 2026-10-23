use inkdb::db::Database;
use inkdb::errors::InkError;
use inkdb::shell::InkShell;
use inkdb::vfs::Vfs;

fn main() -> Result<(), InkError> {
    let database = match std::env::args().nth(1) {
        Some(path) => Some(Database::open_or_create(path)?),
        None => None,
    };
    if let Some(database) = database {
        run(database)?;
    } else {
        /*Temporary for now*/
        let database = Database::new_mem()?;
        run(database)?;
    }
    Ok(())
}
fn run<V: Vfs>(mut database: Database<V>) -> Result<(), InkError> {
    if let Some(q) = std::env::args().nth(2) {
        if let Err(e) = InkShell::run_query(&mut database, &q) {
            println!("Error: {e}")
        }
        return Ok(());
    }
    InkShell::run::<V>(&mut database);
    Ok(())
}

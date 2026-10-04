use inkdb::db::Database;
use inkdb::errors::InkError;
use inkdb::shell::InkShell;

fn main() -> Result<(), InkError> {
    let path = match std::env::args().nth(1) {
        Some(path) => path,
        None => {
            eprintln!(
                "usage: ink <(for sqlite files) database.db | (for inkdb files) database.inkdb>"
            );
            std::process::exit(2);
        }
    };
    let mut database = Database::open_or_create(path)?;
    InkShell::run(&mut database);
    Ok(())
}

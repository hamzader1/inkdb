use thiserror::Error;
#[derive(Debug, Error)]
pub enum SqliteDatabaseError {
    #[error("Failed to open database file: {0}")]
    DatabaseOpenFailure(#[from] std::io::Error),

    #[error("Invalid SQLite database header")]
    InvalidDatabaseHeader,
}

use thiserror::Error;
#[derive(Debug, Error)]
pub enum SqliteDatabaseError {
    #[error("Failed to open database file: {0}")]
    DatabaseOpenFailure(#[from] std::io::Error),

    #[error("Invalid SQLite database header")]
    InvalidDatabaseHeader,

    #[error("Unsupported SQLite file format version: {0}")]
    UnsupportedFileFormat(u32),

    #[error("Invalid page size: {0}")]
    InvalidPageSize(u16),

    #[error("Database appears to be corrupted")]
    DatabaseCorrupted,

    #[error("Invalid page number: {0}")]
    InvalidPageNumber(u32),

    #[error("Page {page} is corrupted: {reason}")]
    CorruptedPage { page: u32, reason: String },

    #[error("Page data is corrupted")]
    CorruptedPageData,

    #[error("Invalid page type: 0x{0:02X}")]
    InvalidPageType(u8),

    #[error("Cell pointer is out of bounds: {0}")]
    InvalidCellPointer(u16),

    #[error("Cell content overlaps page header or pointer array")]
    CellOverlap,

    #[error("Cell count is inconsistent with the page layout")]
    InvalidCellCount,

    #[error("Malformed cell")]
    MalformedCell,

    #[error("Invalid record header")]
    InvalidRecordHeader,

    #[error("Invalid serial type: {0}")]
    InvalidSerialType(u64),

    #[error("Malformed varint")]
    InvalidVarint,

    #[error("Unexpected end of varint")]
    UnexpectedEndOfVarint,

    #[error("{0}")]
    Corrupt(String),
}

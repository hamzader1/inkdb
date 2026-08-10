use std::rc::Rc;
use thiserror::Error;
#[derive(Debug, Error)]
pub enum SqliteError {
    #[error("Failed to open database file: {0}")]
    DatabaseOpenFailure(#[from] std::io::Error),

    #[error("Database not exists")]
    DatabaseNotExists,

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

    #[error("buffer pool exhausted: no unpinned frame available for eviction")]
    BufferPoolExhausted,

    #[error("{0}")]
    OverFlow(String),

    #[error("Invalid number from position {start} to {end}: Number too large or malformed")]
    InvalidNumber {
        input: String,
        start: usize,
        end: usize,
    },

    #[error(
            "Unexpected Character '{character}' at position {position}: Expected alphanumeric, operator, or keyword"
        )]
    UnexpectedChar {
        input: String,
        character: char,
        position: usize,
    },

    #[error("Unterminated string at position {position}: Expected closing quote")]
    UnterminatedString { input: String, position: usize },

    #[error("Unclosed parenthesis at position {position}: Expected ')'")]
    UnterminatedParenthsis { input: String, position: usize },

    #[error("Unmatched ')' at position {position}: No matching '(' found")]
    UnmatchedClosingParenthesis { input: String, position: usize },
}

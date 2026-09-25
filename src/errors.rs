use thiserror::Error;

use crate::pager::pager::PageNo;
use crate::sql::tokens::{Span, TokenKind};
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

    #[error(transparent)]
    Corrupt(#[from] CorruptError),

    #[error("internal error: {0} (this is a bug, please report it)")]
    Internal(&'static str),

    #[error("internal error: {0} (this is a bug, please report it)")]
    InternalFmt(String),

    #[error("table '{0}' already exists")]
    TableAlreadyExists(String),

    #[error("table '{0}' does not exist")]
    TableNotFound(String),

    #[error("column '{0}' does not exist")]
    UnknownColumn(String),

    #[error("unsupported statement: {0}")]
    Unsupported(String),

    #[error("file range out of bounds: {0}")]
    FileRange(String),

    #[error("buffer pool exhausted: no unpinned frame available for eviction")]
    BufferPoolExhausted,

    #[error("{0}")]
    Overflow(String),

    #[error(transparent)]
    Runtime(#[from] RuntimeError),

    #[error(transparent)]
    Syntax(#[from] SyntaxError),

    #[error("A transaction is already active")]
    TransactionAlreadyStarted,

    #[error("No active transaction found")]
    NoActiveTransaction,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("{0}")]
    Message(String),
    #[error("cannot convert {actual} value to {expected}")]
    TypeConversion {
        expected: &'static str,
        actual: &'static str,
    },
}

#[derive(Debug, Error)]
pub enum CorruptError {
    #[error("an invariant check failed: {0}")]
    Assertion(String),
    #[error("sqlite_master record has {columns} columns, expected 5")]
    CatalogRecord { columns: usize },
    #[error("index predecessor leaf is empty")]
    EmptyPredecessorLeaf,
    #[error("freelist trunk page is nonzero but freelist count is zero")]
    FreelistCountMissing,
    #[error("trunk page is page 1")]
    FreelistPageIsHeader,
    #[error("freelist count is nonzero but first trunk page is zero")]
    FreelistTrunkMissing,
    #[error("leaf page is page 1")]
    FreelistLeafIsHeader,
    #[error("index root {index_page}: entry missing for a row being deleted")]
    IndexEntryMissing { index_page: PageNo },
    #[error("index {index_page} holds rowid {rowid} but table {table_page} has no such row")]
    IndexEntryWithoutRow {
        index_page: PageNo,
        rowid: u64,
        table_page: PageNo,
    },
    #[error("invalid overflow page pointer")]
    InvalidOverflowPointer,
    #[error("page {page} has no right-most child")]
    MissingRightMostChild { page: PageNo },
    #[error("failed to parse the next overflow page")]
    OverflowNextPointer,
    #[error("assembled payload length mismatch")]
    OverflowPayloadMismatch,
    #[error("local payload exceeds total payload length")]
    OverflowPayloadTooLong,
    #[error("parent {parent} slot {slot} points at page {points_at}, expected {expected}")]
    ParentSlotMismatch {
        parent: PageNo,
        slot: u16,
        points_at: PageNo,
        expected: PageNo,
    },
    #[error("replace_cell: page refused its own old cell")]
    ReplaceCellRefused,
    #[error("row vanished between exact seek and read")]
    RowVanished,
    #[error("page {page} is not a {expected} page")]
    UnexpectedPageKind {
        page: PageNo,
        expected: &'static str,
    },
    #[error("invalid left child page number: 0")]
    ZeroChildPointer,
}

#[derive(Debug, Error)]
#[error("{kind} at {span:?}")]
pub struct SyntaxError {
    pub kind: SyntaxErrorKind,
    pub span: Span,
}

impl SqliteError {
    pub fn runtime(message: impl Into<String>) -> Self {
        Self::Runtime(RuntimeError::Message(message.into()))
    }

    pub fn type_conversion(expected: &'static str, actual: &'static str) -> Self {
        Self::Runtime(RuntimeError::TypeConversion { expected, actual })
    }

    pub fn syntax(kind: SyntaxErrorKind, span: Span) -> Self {
        Self::Syntax(SyntaxError { kind, span })
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum SyntaxErrorKind {
    #[error("invalid number: too large or malformed")]
    InvalidNumber,
    #[error("unexpected character '{0}': expected alphanumeric, operator or keyword")]
    UnexpectedChar(char),
    #[error("unterminated string: expected a closing quote")]
    UnterminatedString,
    #[error("unclosed parenthesis: expected ')'")]
    UnclosedParenthesis,
    #[error("unmatched ')': no matching '(' found")]
    UnmatchedClosingParenthesis,
    #[error("expected {expected} token, got {actual}")]
    TokenMismatch {
        expected: TokenKind,
        actual: TokenKind,
    },
    #[error("unexpected end of expression, expected {0}")]
    UnexpectedEndOfExpression(TokenKind),
    #[error("expected an identifier, got a {0} token")]
    ExpectedIdentifier(TokenKind),
}

pub fn render_syntax_error(sql: &str, err: &SqliteError) -> Option<String> {
    let SqliteError::Syntax(syntax) = err else {
        return None;
    };
    let Span(start, end) = syntax.span;
    let caret_len = end.saturating_sub(start).max(1);
    let pointer = format!("{}{}", " ".repeat(start + 1), "^".repeat(caret_len));
    Some(format!(
        "{}
	 {}
	{}
",
        syntax.kind, sql, pointer
    ))
}

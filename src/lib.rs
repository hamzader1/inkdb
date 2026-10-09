pub mod backend;
pub mod db;
pub mod record;
mod schema;
pub mod shell;
pub mod sql;
pub use schema::Master;
pub mod errors;

pub mod pager;
pub mod storage;
pub(crate) mod util;

pub(crate) mod varint;
pub mod vfs;
use errors::InkError;

pub(crate) use storage::mem_cursor::MemCursor;

pub(crate) type InkResult<T> = Result<T, InkError>;

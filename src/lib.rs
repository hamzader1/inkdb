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
mod util;

mod varint;
pub mod vfs;
use errors::InkError;

pub(crate) use storage::cursor::MemCursor;

pub type Result<T, E = InkError> = std::result::Result<T, E>;

pub type InkResult<T> = Result<T, InkError>;

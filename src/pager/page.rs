use super::buffer_pool::BufferPool;
use crate::format::page::PageNo;
use crate::vfs::file::SqliteFile;

type FrameId = usize;

struct Pager<F: SqliteFile> {
    source: F,
    buffer_pool: BufferPool,
}

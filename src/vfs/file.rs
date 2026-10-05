#[expect(unused)]
use crate::vfs::Vfs;
use crate::{InkError, InkResult};
use std::path::PathBuf;

/// An open file inside a VFS, produced by [`Vfs::open`].
///
/// Where the VFS is the factory, an `InkFile` is the handle: the thing the
/// pager reads and writes through. Implementations are free to back it however
/// they like, so the same engine code works against a real disk file
/// ([`DiskFile`](crate::vfs::disk::DiskFile))
/// or an in-memory buffer
/// ([`MemFile`](crate::vfs::mem::MemFile))).
#[allow(clippy::len_without_is_empty)]
pub trait InkFile: std::fmt::Debug {
    /// The name of the file, without any directory part.
    fn name(&self) -> &str;

    /// The directory the file lives in, used to place neighbour files such as
    /// the journal next to it.
    fn path(&self) -> PathBuf;

    /// The length of the file in bytes.
    fn len(&self) -> Result<u64, InkError>;

    /// Read exactly `buff.len()` bytes starting at `offset` into `buff`.
    ///
    /// # Errors
    /// [`InkError::FileRange`] when `offset + buff.len()` runs past the end of
    /// the file. Growing the file with [`InkFile::set_len`] comes first.
    fn read_exact_at(&self, offset: u64, buff: &mut [u8]) -> Result<(), InkError>;

    /// Write `buff` at `offset`, leaving the bytes outside that range alone.
    ///
    /// # Errors
    /// [`InkError::FileRange`] when `offset + buff.len()` runs past the end of
    /// the file, so a write can never silently extend it.
    fn write_all_at(&self, offset: u64, buff: &[u8]) -> Result<(), InkError>;

    /// Append `buff` at the end of the file.
    fn write_all(&mut self, buff: &[u8]) -> InkResult<()>;

    /// Resize the file to `len` bytes, zero filling any growth.
    fn set_len(&self, len: usize) -> Result<(), InkError>;

    /// Flush every change to persistent storage.
    fn sync(&self) -> Result<(), InkError>;
}

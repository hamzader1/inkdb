use std::path::Path;

use self::file::InkFile;
use crate::{InkError, InkResult};
#[expect(unused)]
use disk::DiskVfs;
#[expect(unused)]
use mem::MemVfs;
#[expect(unused)]
use std::fs::OpenOptions;

pub mod cursor;
pub mod disk;
pub mod file;
pub mod mem;
mod temp;

/// Flag bit for [`InkOptions`]: the file may be read.
const READ: u8 = 1 << 0;
/// Flag bit for [`InkOptions`]: the file may be written.
const WRITE: u8 = 1 << 1;
/// Flag bit for [`InkOptions`]: the file is created when it is missing.
const CREATE: u8 = 1 << 2;
/// File access flags interpreted by each VFS implementation.
pub struct InkOptions {
    /// The open flags, a combination of `READ`, `WRITE` and `CREATE`.
    options: u8,
}

/// Abstracts database, journal, and temporary file operations.
///
/// [`DiskVfs`] accesses files on disk. `MemVfs` keeps them in memory and is
/// primarily used in tests.
pub trait Vfs: std::fmt::Debug {
    type File: InkFile;

    /// Open a file, creating it when the `CREATE` flag is set, and return the
    /// VFS's [`InkFile`] handle for it.
    fn open<F: AsRef<Path>>(&mut self, f: F, options: InkOptions) -> Result<Self::File, InkError>;
    /// Open the journal that lives next to `db`, creating it when it is not
    /// there yet.
    fn open_journal(&mut self, db: &Self::File) -> Result<Self::File, InkError>;
    /// Delete the journal once it is no longer needed.
    fn delete_journal(&mut self, db: &Self::File) -> Result<(), InkError>;
    /// Read the whole journal into a buffer, or `None` when there is no journal.
    fn read_journal(&self, db: &Self::File) -> Result<Option<Vec<u8>>, InkError>;
    /// Open a temporary file in the system temporary location.
    ///
    /// Used to hold intermediate results that do not fit in memory.
    fn open_temp<T: AsRef<Path>>(&mut self, name: T) -> InkResult<Self::File>;
    /// Remove a temporary file.
    ///
    /// A missing file is not an error: cleanup runs even when the file was
    /// never created.
    fn remove_temp<T: AsRef<Path>>(&mut self, name: T) -> InkResult<()>;
}
impl InkOptions {
    /// Start from no flags at all: no read, no write, no create.
    pub fn new() -> Self {
        Self { options: 0x00 }
    }

    /// Turn a single flag on or off.
    ///
    /// Panics when `flag` is not one of `READ`, `WRITE` or `CREATE`, because
    /// anything else would corrupt the bitset.
    pub fn set(&mut self, flag: u8, set_to: bool) {
        assert!(flag == READ || flag == WRITE || flag == CREATE);
        if set_to {
            self.options |= flag;
        } else {
            self.options &= !flag;
        }
    }
    /// Set the `READ` flag. Builder form, so calls chain.
    pub fn read(mut self, read: bool) -> Self {
        self.set(READ, read);
        self
    }
    /// Set the `WRITE` flag. Builder form, so calls chain.
    pub fn write(mut self, write: bool) -> Self {
        self.set(WRITE, write);
        self
    }
    /// Set the `CREATE` flag. Builder form, so calls chain.
    pub fn create(mut self, create: bool) -> Self {
        self.set(CREATE, create);
        self
    }
    /// Whether the `READ` flag is set.
    pub fn can_read(&self) -> bool {
        self.options & READ != 0
    }

    /// Whether the `WRITE` flag is set.
    pub fn can_write(&self) -> bool {
        self.options & WRITE != 0
    }

    /// Whether the `CREATE` flag is set.
    pub fn is_create(&self) -> bool {
        self.options & CREATE != 0
    }

    /// Set the read, write, and create flags.
    pub fn all() -> Self {
        Self {
            options: READ | WRITE | CREATE,
        }
    }
}

impl Default for InkOptions {
    /// The flags for opening a path that already exists: read and write, but
    /// never create.
    fn default() -> Self {
        Self {
            options: READ | WRITE,
        }
    }
}

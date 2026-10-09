#![allow(dead_code)]
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::file::InkFile;
use super::temp::create_temp_dir;
use crate::InkError;
use crate::vfs::Vfs;

/// The name every in-memory file reports, so the engine always sees a name.
const MEM_B: &str = "__INK_MEMORY_BUFFER";
/// The directory the in-memory files report, which keeps neighbour lookups
/// like the journal's working even though no bytes are ever written.
const MEM_D: &str = "__INK_MEMORY_DIR";

/// The in-memory VFS: a file is a buffer instead of a real file.
///
/// Nothing reaches the disk, which makes tests faster and lets them run the
/// whole engine without cleaning up files afterwards.
///
/// Buffers live in `db_buffers` keyed by path, and each one is shared with the
/// handles opened from it through an [`Rc`]. Journals are kept separately, keyed
/// by the address of the buffer they belong to, so a database and its journal
/// stay paired without needing a file name.
#[derive(Debug, Default)]
pub(crate) struct MemVfs {
    /// One shared buffer per database path, and per temporary file name.
    db_buffers: HashMap<PathBuf, Rc<RefCell<Vec<u8>>>>,
    /// One shared buffer per open database handle, addressed by pointer.
    journals: HashMap<usize, Rc<RefCell<Vec<u8>>>>,
}

impl MemVfs {
    /// Start with no files (buffers) at all.
    pub fn new() -> Self {
        Self {
            db_buffers: HashMap::new(),
            journals: HashMap::new(),
        }
    }

    /// Put `bytes` in the VFS under `f_name`, so a later [`Vfs::open`] of that
    /// path finds them. Tests use this to hand the engine a database built by
    /// someone else.
    pub fn insert<P>(&mut self, f_name: P, bytes: Vec<u8>)
    where
        P: AsRef<Path>,
    {
        self.db_buffers
            .insert(f_name.as_ref().to_path_buf(), Rc::new(RefCell::new(bytes)));
    }
}

impl Vfs for MemVfs {
    type File = MemFile;

    fn open<F: AsRef<Path>>(
        &mut self,
        f: F,
        _options: super::InkOptions,
    ) -> Result<Self::File, InkError> {
        if let Some(bytes) = self.db_buffers.get(&f.as_ref().to_path_buf()) {
            return Ok(MemFile::new(Rc::clone(bytes)));
        }

        Err(InkError::DatabaseNotExists)
    }
    fn open_journal(&mut self, db: &Self::File) -> Result<Self::File, InkError> {
        let key = Rc::as_ptr(&db.bytes) as usize;
        let entry = self
            .journals
            .entry(key)
            .or_insert_with(|| Rc::new(RefCell::new(Vec::new())));
        Ok(MemFile::with_dir(Rc::clone(entry), db.temp_dir.clone()))
    }
    fn delete_journal(&mut self, db: &Self::File) -> Result<(), InkError> {
        let key = Rc::as_ptr(&db.bytes) as usize;
        self.journals.remove(&key);
        Ok(())
    }
    fn read_journal(&self, db: &Self::File) -> Result<Option<Vec<u8>>, InkError> {
        let key = Rc::as_ptr(&db.bytes) as usize;
        Ok(self.journals.get(&key).map(|b| b.borrow().clone()))
    }
    fn open_temp<T: AsRef<Path>>(&mut self, name: T) -> crate::InkResult<Self::File> {
        let entry = self
            .db_buffers
            .entry(name.as_ref().to_path_buf())
            .or_insert_with(|| Rc::new(RefCell::new(Vec::new())));
        Ok(MemFile::new(Rc::clone(entry)))
    }
    fn remove_temp<T: AsRef<Path>>(&mut self, name: T) -> crate::InkResult<()> {
        self.db_buffers.remove(&name.as_ref().to_path_buf());
        Ok(())
    }
}

/// A file whose bytes live in memory.
///
/// The buffer is shared, not owned, so reopening the same path hands out another
/// handle onto the same bytes, exactly like a path on disk.
#[derive(Debug)]
pub(crate) struct MemFile {
    bytes: Rc<RefCell<Vec<u8>>>,
    temp_dir: PathBuf,
}

impl MemFile {
    /// Wrap `bytes` and give the handle a temporary directory of its own, so
    /// [`InkFile::path`] has something real to return.
    pub(crate) fn new(bytes: Rc<RefCell<Vec<u8>>>) -> Self {
        let path =
            create_temp_dir(MEM_D).expect("Error while trying to create a temporary memory dir");
        Self {
            bytes,
            temp_dir: path,
        }
    }
    /// Wrap `bytes` and reuse an existing directory instead of creating one.
    /// Journals take this path so they sit beside the database they belong to.
    pub(crate) fn with_dir(bytes: Rc<RefCell<Vec<u8>>>, temp_dir: PathBuf) -> Self {
        Self { bytes, temp_dir }
    }
}

impl InkFile for MemFile {
    fn name(&self) -> &str {
        MEM_B
    }
    fn path(&self) -> PathBuf {
        self.temp_dir.clone()
    }
    fn len(&self) -> Result<u64, InkError> {
        Ok(self.bytes.borrow().len() as u64)
    }

    fn read_exact_at(&self, offset: u64, buff: &mut [u8]) -> Result<(), InkError> {
        let bytes = self.bytes.borrow();

        let start = offset as usize;
        let end = start + buff.len();

        if end > bytes.len() {
            return Err(InkError::FileRange(format!(
                "read of {} bytes at offset {offset} exceeds buffer length {}",
                buff.len(),
                bytes.len()
            )));
        }

        buff.copy_from_slice(&bytes[start..end]);

        Ok(())
    }

    fn write_all_at(&self, offset: u64, buff: &[u8]) -> Result<(), InkError> {
        let mut bytes = self.bytes.borrow_mut();

        let start = offset as usize;
        let end = start + buff.len();

        if end > bytes.len() {
            return Err(InkError::FileRange(format!(
                "write of {} bytes at offset {offset} exceeds buffer length {}",
                buff.len(),
                bytes.len()
            )));
        }

        bytes[start..end].copy_from_slice(buff);

        Ok(())
    }
    fn write_all(&mut self, buff: &[u8]) -> crate::InkResult<()> {
        let mut bytes = self.bytes.borrow_mut();
        bytes.extend_from_slice(buff);
        Ok(())
    }

    fn set_len(&self, len: usize) -> Result<(), InkError> {
        self.bytes.borrow_mut().resize(len, 0);
        Ok(())
    }

    fn sync(&self) -> Result<(), InkError> {
        // Nothing to do for an in-memory buffer.
        Ok(())
    }
}

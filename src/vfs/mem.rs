use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::file::SqliteFile;
use super::temp::create_temp_dir;
use crate::DbError;
use crate::errors::SqliteError;
use crate::vfs::Vfs;

const MEM_B: &str = "__INK_MEMORY_BUFFER";
const MEM_D: &str = "__INK_MEMORY_DIR";

#[derive(Debug, Default)]
pub struct MemVfs {
    db_buffers: HashMap<PathBuf, Rc<RefCell<Vec<u8>>>>,
}

impl MemVfs {
    pub fn new() -> Self {
        Self {
            db_buffers: HashMap::new(),
        }
    }

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
        _options: super::SqliteOptions,
    ) -> Result<Self::File, DbError> {
        if let Some(bytes) = self.db_buffers.get(&f.as_ref().to_path_buf()) {
            return Ok(MemFile::new(Rc::clone(bytes)));
        }

        Err(DbError::DatabaseNotExists)
    }
}

#[derive(Debug)]
pub struct MemFile {
    bytes: Rc<RefCell<Vec<u8>>>,
    temp_dir: PathBuf,
}

impl MemFile {
    pub fn new(bytes: Rc<RefCell<Vec<u8>>>) -> Self {
        let path =
            create_temp_dir(MEM_D).expect("Error while trying to create a temporary memory dir");
        Self {
            bytes,
            temp_dir: path,
        }
    }
}

impl SqliteFile for MemFile {
    fn name(&self) -> &str {
        MEM_B
    }
    fn path(&self) -> PathBuf {
        self.temp_dir.clone()
    }
    fn len(&self) -> Result<u64, DbError> {
        Ok(self.bytes.borrow().len() as u64)
    }

    fn read_exact_at(&self, offset: u64, buff: &mut [u8]) -> Result<(), DbError> {
        let bytes = self.bytes.borrow();

        let start = offset as usize;
        let end = start + buff.len();

        if end > bytes.len() {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }

        buff.copy_from_slice(&bytes[start..end]);

        Ok(())
    }

    fn write_all_at(&self, offset: u64, buff: &[u8]) -> Result<(), DbError> {
        let mut bytes = self.bytes.borrow_mut();

        let start = offset as usize;
        let end = start + buff.len();

        if end > bytes.len() {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }

        bytes[start..end].copy_from_slice(buff);

        Ok(())
    }

    fn set_len(&self, len: usize) -> Result<(), DbError> {
        self.bytes.borrow_mut().resize(len, 0);
        Ok(())
    }

    fn sync(&self) -> Result<(), DbError> {
        // Nothing to do for an in-memory buffer.
        Ok(())
    }
}

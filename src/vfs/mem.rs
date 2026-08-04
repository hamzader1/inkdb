use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::DbError;
use crate::errors::SqliteError;
use crate::vfs::Vfs;

use super::file::SqliteFile;
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
        options: super::SqliteOptions,
    ) -> Result<Self::File, crate::DbError> {
        if let Some(bytes) = self.db_buffers.get(&f.as_ref().to_path_buf()) {
            return Ok(MemFile::new(Rc::clone(bytes)));
        }
        Err(DbError::DatabaseNotExists)
    }
}

#[derive(Debug)]
pub struct MemFile {
    bytes: Rc<RefCell<Vec<u8>>>,
}

impl MemFile {
    pub fn new(bytes: Rc<RefCell<Vec<u8>>>) -> Self {
        Self { bytes }
    }
}
impl SqliteFile for MemFile {
    fn len(&self) -> Result<u64, crate::DbError> {
        Ok(self.bytes.borrow().len() as _)
    }
    fn read_exact_at<B: AsMut<[u8]> + ?Sized>(
        &self,
        offset: u64,
        buff: &mut B,
    ) -> Result<(), crate::DbError> {
        let bytes = &self.bytes.borrow();
        let buf = buff.as_mut();
        let start = offset as usize;
        let end = start + buf.len();
        if end > bytes.len() {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }
        let slice = &bytes[start..end];
        buf.copy_from_slice(slice);
        Ok(())
    }

    fn write_all_at<B: AsRef<[u8]> + ?Sized>(
        &self,
        offset: u64,
        buff: &B,
    ) -> Result<(), crate::DbError> {
        let bytes = &mut self.bytes.borrow_mut();
        let buff = buff.as_ref();
        let start = offset as usize;
        let end = start + buff.len();
        if end > bytes.len() {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }
        let slice = &mut bytes[start..end];
        slice.copy_from_slice(buff);
        Ok(())
    }
    fn set_len(&self, len: usize) -> Result<(), crate::DbError> {
        unsafe {
            self.bytes.borrow_mut().set_len(len);
        }
        Ok(())
    }
    fn sync(&self) -> Result<(), crate::DbError> {
        // Empty: Nothing we can for in memory buffers
        Ok(())
    }
}

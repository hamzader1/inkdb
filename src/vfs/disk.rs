use crate::DbError;

use super::file::SqliteFile;
use super::{SqliteOptions, Vfs};
use std::fs::OpenOptions;
use std::os::unix::fs::FileExt;

#[derive(Debug)]
pub struct DiskFvs;

#[derive(Debug)]
pub struct DiskFile {
    file: std::fs::File,
}

impl Vfs for DiskFvs {
    type File = DiskFile;
    fn open<F: AsRef<std::path::Path>>(
        &mut self,
        f: F,
        options: super::SqliteOptions,
    ) -> Result<Self::File, crate::DbError> {
        let options = OpenOptions::from(options);
        let file = options.open(f)?;
        Ok(DiskFile { file })
    }
}

impl SqliteFile for DiskFile {
    fn len(&self) -> Result<u64, DbError> {
        let len = self.file.metadata()?.len();
        Ok(len)
    }
    fn read_exact_at<B: AsMut<[u8]> + ?Sized>(
        &self,
        offset: u64,
        buff: &mut B,
    ) -> Result<(), DbError> {
        self.file.read_exact_at(buff.as_mut(), offset)?;
        Ok(())
    }
    fn write_all_at<B: AsRef<[u8]> + ?Sized>(&self, offset: u64, buff: &B) -> Result<(), DbError> {
        self.file.write_all_at(buff.as_ref(), offset)?;
        Ok(())
    }

    fn set_len(&self, len: usize) -> Result<(), DbError> {
        self.file.set_len(len as u64)?;
        Ok(())
    }
    fn sync(&self) -> Result<(), DbError> {
        // TODO:
        self.file.sync_all();
        Ok(())
    }
}

impl From<SqliteOptions> for OpenOptions {
    fn from(value: SqliteOptions) -> Self {
        let mut options = OpenOptions::new();
        options.read(value.can_read());
        options.write(value.can_write());
        options.create(value.is_create());
        options
    }
}

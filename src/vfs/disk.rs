use crate::DbError;
#[cfg(unix)]
use crate::errors::SqliteError;

use super::file::SqliteFile;
use super::{SqliteOptions, Vfs};
use std::fs::OpenOptions;

#[cfg(unix)]
use std::os::unix::fs::FileExt;

#[cfg(windows)]
use std::os::windows::fs::FileExt;
use std::path::PathBuf;

#[derive(Debug)]
pub struct DiskVfs;

#[derive(Debug)]
pub struct DiskFile {
    pub file: std::fs::File,
    pub path: PathBuf,
}

impl Vfs for DiskVfs {
    type File = DiskFile;

    fn open<F: AsRef<std::path::Path>>(
        &mut self,
        f: F,
        options: super::SqliteOptions,
    ) -> Result<Self::File, crate::DbError> {
        let options = OpenOptions::from(options);
        let file = options.open(&f)?;
        Ok(DiskFile {
            file,
            path: f.as_ref().to_path_buf(),
        })
    }
}

#[cfg(unix)]
impl SqliteFile for DiskFile {
    fn path(&self) -> PathBuf {
        self.path.parent().unwrap().to_path_buf()
    }

    fn name(&self) -> &str {
        self.path
            .file_name()
            .unwrap()
            .to_str()
            .expect("Error while trying to convert OsStr")
    }
    fn len(&self) -> Result<u64, DbError> {
        let len = self.file.metadata()?.len();
        Ok(len)
    }

    fn read_exact_at(&self, offset: u64, buff: &mut [u8]) -> Result<(), DbError> {
        let file_len = self.file.metadata()?.len();

        if (offset as usize) + buff.len() > file_len as usize {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }

        self.file.read_exact_at(buff, offset)?;
        Ok(())
    }

    fn write_all_at(&self, offset: u64, buff: &[u8]) -> Result<(), DbError> {
        let file_len = self.file.metadata()?.len();

        if (offset as usize) + buff.len() > file_len as usize {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }

        self.file.write_all_at(buff, offset)?;
        Ok(())
    }

    fn set_len(&self, len: usize) -> Result<(), DbError> {
        self.file.set_len(len as u64)?;
        Ok(())
    }

    fn sync(&self) -> Result<(), DbError> {
        self.file.sync_all()?;
        Ok(())
    }
}

#[cfg(windows)]
impl SqliteFile for DiskFile {
    fn path(&self) -> PathBuf {
        self.path.parent().unwrap().to_path_buf()
    }

    fn name(&self) -> &str {
        self.path
            .file_name()
            .unwrap()
            .to_str()
            .expect("Error while trying to convert OsStr")
    }
    fn len(&self) -> Result<u64, DbError> {
        let len = self.file.metadata()?.len();
        Ok(len)
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), DbError> {
        use crate::SqliteError;

        let file_len = self.file.metadata()?.len();

        if offset as usize + buf.len() > file_len as usize {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }

        let mut offset = offset;
        let mut buf = buf;

        while !buf.is_empty() {
            let n = self.file.seek_read(buf, offset)?;

            if n == 0 {
                return Err(DbError::Corrupt("Unexpected EOF".into()));
            }

            offset += n as u64;
            buf = &mut buf[n..];
        }

        Ok(())
    }

    fn write_all_at(&self, offset: u64, buf: &[u8]) -> Result<(), DbError> {
        use crate::SqliteError;

        let file_len = self.file.metadata()?.len();

        if offset as usize + buf.len() > file_len as usize {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }

        let mut offset = offset;
        let mut buf = buf;

        while !buf.is_empty() {
            let n = self.file.seek_write(buf, offset)?;

            if n == 0 {
                return Err(DbError::Corrupt("failed to write the whole buffer".into()));
            }

            offset += n as u64;
            buf = &buf[n..];
        }

        Ok(())
    }

    fn set_len(&self, len: usize) -> Result<(), DbError> {
        self.file.set_len(len as u64)?;
        Ok(())
    }

    fn sync(&self) -> Result<(), DbError> {
        self.file.sync_all()?;
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

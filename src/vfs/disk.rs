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

#[derive(Debug)]
pub struct DiskVfs;

#[derive(Debug)]
pub struct DiskFile {
    pub file: std::fs::File,
}

impl Vfs for DiskVfs {
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

#[cfg(unix)]
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
        let file_len = self.file.metadata()?.len();
        let buf = buff.as_mut();
        if (offset as usize) + buf.len() > file_len as _ {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }
        self.file.read_exact_at(buf, offset)?;
        Ok(())
    }
    fn write_all_at<B: AsRef<[u8]> + ?Sized>(&self, offset: u64, buff: &B) -> Result<(), DbError> {
        let file_len = self.file.metadata()?.len();
        let buf = buff.as_ref();
        if (offset as usize) + buf.len() > file_len as _ {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }
        self.file.write_all_at(buff.as_ref(), offset)?;
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
    fn len(&self) -> Result<u64, DbError> {
        let len = self.file.metadata()?.len();
        Ok(len)
    }
    fn read_exact_at<B: AsMut<[u8]> + ?Sized>(
        &self,
        offset: u64,
        buf: &mut B,
    ) -> Result<(), DbError> {
        use crate::SqliteError;
        let file_len = self.file.metadata()?.len();
        let mut buf = buf.as_mut();
        if offset as usize + buf.len() > file_len as _ {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }
        let mut offset = offset;
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
    fn write_all_at<B: AsRef<[u8]> + ?Sized>(&self, offset: u64, buf: &B) -> Result<(), DbError> {
        use crate::SqliteError;
        let file_len = self.file.metadata()?.len();
        let mut buf = buf.as_ref();
        if offset as usize + buf.len() > file_len as _ {
            return Err(SqliteError::Corrupt("Range out of bounds".into()));
        }
        let mut offset = offset;
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

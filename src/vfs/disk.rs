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

impl DiskVfs {
    fn journal_path(db: &DiskFile) -> PathBuf {
        let name = db.name().to_owned() + "-journal";
        db.path().join(name)
    }
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
    fn open_journal(&mut self, db: &Self::File) -> Result<Self::File, crate::DbError> {
        self.open(Self::journal_path(db), super::SqliteOptions::all())
    }
    fn delete_journal(&mut self, db: &Self::File) -> Result<(), crate::DbError> {
        match std::fs::remove_file(Self::journal_path(db)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    fn read_journal(&self, db: &Self::File) -> Result<Option<Vec<u8>>, crate::DbError> {
        let path = Self::journal_path(db);
        if !path.exists() {
            return Ok(None);
        }
        let mut vfs = DiskVfs;
        let file = vfs.open(path, super::SqliteOptions::default())?;
        let len = file.len()?;
        let mut bytes = vec![0u8; len as _];
        file.read_exact_at(0, &mut bytes)?;
        Ok(Some(bytes))
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
            return Err(SqliteError::FileRange(format!(
                "read of {} bytes at offset {offset} exceeds file length {file_len} (grow the file with set_len first)",
                buff.len()
            )));
        }

        self.file.read_exact_at(buff, offset)?;
        Ok(())
    }

    fn write_all_at(&self, offset: u64, buff: &[u8]) -> Result<(), DbError> {
        let file_len = self.file.metadata()?.len();

        if (offset as usize) + buff.len() > file_len as usize {
            return Err(SqliteError::FileRange(format!(
                "write of {} bytes at offset {offset} exceeds file length {file_len} (grow the file with set_len first)",
                buff.len()
            )));
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
            return Err(SqliteError::FileRange(
                format!(
                    "read of {} bytes at offset {offset} exceeds file length {file_len}",
                    buf.len()
                )
                .into(),
            ));
        }

        let mut offset = offset;
        let mut buf = buf;

        while !buf.is_empty() {
            let n = self.file.seek_read(buf, offset)?;

            if n == 0 {
                return Err(DbError::FileRange(
                    format!("unexpected EOF reading at offset {offset}").into(),
                ));
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
            return Err(SqliteError::FileRange(
                format!(
                    "write of {} bytes at offset {offset} exceeds file length {file_len}",
                    buf.len()
                )
                .into(),
            ));
        }

        let mut offset = offset;
        let mut buf = buf;

        while !buf.is_empty() {
            let n = self.file.seek_write(buf, offset)?;

            if n == 0 {
                return Err(DbError::FileRange(
                    format!("failed to write the whole buffer at offset {offset}").into(),
                ));
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

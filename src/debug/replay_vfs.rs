#[cfg(feature = "debug-instrument")]
use crate::DbError;
#[cfg(feature = "debug-instrument")]
use crate::vfs::file::SqliteFile;
#[cfg(feature = "debug-instrument")]
use std::cell::{Cell, RefCell};
#[cfg(feature = "debug-instrument")]
use std::fs::File;
#[cfg(feature = "debug-instrument")]
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(feature = "debug-instrument")]
use std::path::Path;

#[cfg(feature = "debug-instrument")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ReplayEvent {
    Read {
        offset: u64,
        len: usize,
        data: Vec<u8>,
    },
    Write {
        offset: u64,
        data: Vec<u8>,
    },
    SetLen {
        len: usize,
    },
    Sync,
}

#[cfg(feature = "debug-instrument")]
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ReplayLog {
    pub events: Vec<ReplayEvent>,
}

#[cfg(feature = "debug-instrument")]
impl ReplayLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_read(&mut self, offset: u64, buf: &[u8]) {
        self.events.push(ReplayEvent::Read {
            offset,
            len: buf.len(),
            data: buf.to_vec(),
        });
    }

    pub fn record_write(&mut self, offset: u64, data: &[u8]) {
        self.events.push(ReplayEvent::Write {
            offset,
            data: data.to_vec(),
        });
    }

    pub fn record_set_len(&mut self, len: usize) {
        self.events.push(ReplayEvent::SetLen { len });
    }

    pub fn record_sync(&mut self) {
        self.events.push(ReplayEvent::Sync);
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let file = File::create(path)?;
        serde_json::to_writer(file, self)?;
        Ok(())
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let log: Self = serde_json::from_reader(file)?;
        Ok(log)
    }
}

#[cfg(feature = "debug-instrument")]
pub struct ReplayRecorder {
    log: RefCell<ReplayLog>,
    inner: Box<dyn SqliteFile>,
}

#[cfg(feature = "debug-instrument")]
impl ReplayRecorder {
    pub fn new(inner: Box<dyn SqliteFile>) -> Self {
        Self {
            log: RefCell::new(ReplayLog::new()),
            inner,
        }
    }

    pub fn save(self, path: &Path) -> std::io::Result<()> {
        self.log.borrow().save(path)
    }
}

#[cfg(feature = "debug-instrument")]
impl SqliteFile for ReplayRecorder {
    fn len(&self) -> Result<u64, DbError> {
        self.inner.len()
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), DbError> {
        self.inner.read_exact_at(offset, buf)?;
        self.log.borrow_mut().record_read(offset, buf);
        Ok(())
    }

    fn write_all_at(&self, offset: u64, buf: &[u8]) -> Result<(), DbError> {
        self.inner.write_all_at(offset, buf)?;
        self.log.borrow_mut().record_write(offset, buf);
        Ok(())
    }

    fn set_len(&self, len: usize) -> Result<(), DbError> {
        self.inner.set_len(len)?;
        self.log.borrow_mut().record_set_len(len);
        Ok(())
    }

    fn sync(&self) -> Result<(), DbError> {
        self.inner.sync()?;
        self.log.borrow_mut().record_sync();
        Ok(())
    }
}

#[cfg(feature = "debug-instrument")]
pub struct ReplayPlayer {
    log: ReplayLog,
    index: Cell<usize>,
    inner: Box<dyn SqliteFile>,
}

#[cfg(feature = "debug-instrument")]
impl ReplayPlayer {
    pub fn new(inner: Box<dyn SqliteFile>, log_path: &Path) -> std::io::Result<Self> {
        let log = ReplayLog::load(log_path)?;
        Ok(Self {
            log,
            index: Cell::new(0),
            inner,
        })
    }
}

#[cfg(feature = "debug-instrument")]
impl SqliteFile for ReplayPlayer {
    fn len(&self) -> Result<u64, DbError> {
        self.inner.len()
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), DbError> {
        let idx = self.index.get();
        if idx >= self.log.events.len() {
            return Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Replay log exhausted",
            ))
            .into());
        }

        match &self.log.events[idx] {
            ReplayEvent::Read {
                offset: o, data, ..
            } => {
                if *o != offset {
                    return Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Replay offset mismatch: expected {}, got {}", o, offset),
                    ))
                    .into());
                }
                if data.len() != buf.len() {
                    return Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "Replay length mismatch: expected {}, got {}",
                            data.len(),
                            buf.len()
                        ),
                    ))
                    .into());
                }
                buf.copy_from_slice(data);
                self.index.set(idx + 1);
                Ok(())
            }
            e => Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Expected Read event, got {:?}", e),
            ))
            .into()),
        }
    }

    fn write_all_at(&self, offset: u64, buf: &[u8]) -> Result<(), DbError> {
        let idx = self.index.get();
        if idx >= self.log.events.len() {
            return Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Replay log exhausted",
            ))
            .into());
        }

        match &self.log.events[idx] {
            ReplayEvent::Write { offset: o, data } => {
                if *o != offset {
                    return Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "Replay write offset mismatch: expected {}, got {}",
                            o, offset
                        ),
                    ))
                    .into());
                }
                if data != buf {
                    return Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Replay write data mismatch",
                    ))
                    .into());
                }
                self.index.set(idx + 1);
                Ok(())
            }
            e => Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Expected Write event, got {:?}", e),
            ))
            .into()),
        }
    }

    fn set_len(&self, len: usize) -> Result<(), DbError> {
        let idx = self.index.get();
        if idx >= self.log.events.len() {
            return Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Replay log exhausted",
            ))
            .into());
        }

        match &self.log.events[idx] {
            ReplayEvent::SetLen { len: l } => {
                if *l != len {
                    return Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Replay set_len mismatch: expected {}, got {}", l, len),
                    ))
                    .into());
                }
                self.index.set(idx + 1);
                Ok(())
            }
            e => Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Expected SetLen event, got {:?}", e),
            ))
            .into()),
        }
    }

    fn sync(&self) -> Result<(), DbError> {
        let idx = self.index.get();
        if idx >= self.log.events.len() {
            return Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Replay log exhausted",
            ))
            .into());
        }

        match &self.log.events[idx] {
            ReplayEvent::Sync => {
                self.index.set(idx + 1);
                Ok(())
            }
            e => Err(crate::SqliteError::DatabaseOpenFailure(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Expected Sync event, got {:?}", e),
            ))
            .into()),
        }
    }
}

#[cfg(feature = "debug-instrument")]
pub fn create_recorder(inner: Box<dyn SqliteFile>) -> ReplayRecorder {
    ReplayRecorder::new(inner)
}

#[cfg(feature = "debug-instrument")]
pub fn create_player(inner: Box<dyn SqliteFile>, log_path: &Path) -> std::io::Result<ReplayPlayer> {
    ReplayPlayer::new(inner, log_path)
}

#[cfg(not(feature = "debug-instrument"))]
pub fn create_recorder(
    inner: Box<dyn crate::vfs::file::SqliteFile>,
) -> Box<dyn crate::vfs::file::SqliteFile> {
    inner
}

#[cfg(not(feature = "debug-instrument"))]
pub fn create_player(
    inner: Box<dyn crate::vfs::file::SqliteFile>,
    _log_path: &std::path::Path,
) -> std::io::Result<Box<dyn crate::vfs::file::SqliteFile>> {
    Ok(inner)
}

use std::path::Path;

use self::file::SqliteFile;
use crate::DbError;

mod disk;
mod file;
mod mem;

const READ: u8 = 1 << 0;
const WRITE: u8 = 1 << 1;
const CREATE: u8 = 1 << 2;

pub struct SqliteOptions {
    options: u8,
}

pub trait Vfs {
    type File: SqliteFile;

    fn open<F: AsRef<Path>>(&mut self, f: F, options: SqliteOptions)
        -> Result<Self::File, DbError>;
}
impl SqliteOptions {
    pub fn new() -> Self {
        Self { options: 0x00 }
    }

    pub fn set(&mut self, flag: u8, set_to: bool) {
        assert!(flag == READ || flag == WRITE || flag == CREATE);
        if set_to {
            self.options |= flag;
        } else {
            self.options &= !flag;
        }
    }
    pub fn read(mut self, read: bool) -> Self {
        self.set(READ, read);
        self
    }
    pub fn write(mut self, write: bool) -> Self {
        self.set(WRITE, write);
        self
    }
    pub fn create(mut self, create: bool) -> Self {
        self.set(CREATE, create);
        self
    }
    pub fn can_read(&self) -> bool {
        self.options & READ != 0
    }

    pub fn can_write(&self) -> bool {
        self.options & WRITE != 0
    }

    pub fn is_create(&self) -> bool {
        self.options & CREATE != 0
    }
}

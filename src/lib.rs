#![allow(warnings)]
mod bytes;
mod errors;
mod format;
mod macros;
mod util;
use errors::SqliteDatabaseError;
use format::header::SqliteDatabaseHeader;
use std::fs::{File, OpenOptions};
use std::path::Path;

pub struct SqliteDatabse {
    file: File,
    header: SqliteDatabaseHeader,
}
impl SqliteDatabse {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, SqliteDatabaseError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(db_path)
            .map_err(|err| SqliteDatabaseError::DatabaseOpenFailure(err))?;
        let header = SqliteDatabaseHeader::parse(&mut file)?;
        Ok(Self { file, header })
    }
    pub fn header(&self) -> &'_ SqliteDatabaseHeader {
        &self.header
    }
}

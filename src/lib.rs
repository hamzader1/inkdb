#![allow(warnings)]
mod bytes;
mod errors;
mod format;
mod macros;
use errors::SqliteDatabaseError;
use format::header::SqliteDatabaseHeader;
use std::fs::{File, OpenOptions};
use std::path::Path;

pub struct SqliteDatabse {
    file: File,
    pub header: SqliteDatabaseHeader,
}
impl SqliteDatabse {
    pub fn new<P: AsRef<Path>>(f_name: P) -> Result<Self, SqliteDatabaseError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(f_name)
            .map_err(|err| SqliteDatabaseError::DatabaseOpenFailure(err))?;
        let header = SqliteDatabaseHeader::parse(&mut file)?;
        Ok(Self { file, header })
    }
}

use crate::{to_int, DbError};

use super::file::SqliteFile;

pub struct FileCursor<'source, S> {
    s: &'source S,
    offset: u64,
}
impl<'source, S: SqliteFile> FileCursor<'source, S> {
    pub fn new(s: &'source S) -> Self {
        Self { s, offset: 0 }
    }
    pub fn with_offset(s: &'source S, offset: u64) -> Self {
        let mut cur = Self::new(s);
        cur.offset = offset;
        cur
    }
    pub fn set_offset(&mut self, offset: u64) {
        self.offset = offset
    }

    // custom size buff
    pub fn read_next_exact<B: AsMut<[u8]> + ?Sized>(&mut self, buf: &mut B) -> Result<(), DbError> {
        self.s.read_exact_at(self.offset, buf)?;
        self.offset += buf.as_mut().len() as u64;
        Ok(())
    }

    // ported from /../bytes.rs
    pub fn read_next_u32(&mut self) -> Result<u32, DbError> {
        let mut buf = [0u8; 4];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u32, buf))
    }

    pub fn read_next_u16(&mut self) -> Result<u16, DbError> {
        let mut buf = [0u8; 2];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u16, buf))
    }

    pub fn read_next_u8(&mut self) -> Result<u8, DbError> {
        let mut buf = [0u8; 1];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u8, buf))
    }

    pub fn read_next_arrary<const N: usize>(&mut self) -> Result<[u8; N], DbError> {
        let mut buf = [0u8; N];
        self.read_next_exact(&mut buf)?;
        Ok(buf)
    }
}

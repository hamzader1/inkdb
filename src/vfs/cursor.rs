use super::file::InkFile;
use crate::InkError;
use crate::to_int;

pub(crate) struct FileCursor<'source, S: ?Sized> {
    s: &'source S,
    offset: u64,
}

impl<'source, S: ?Sized + InkFile> FileCursor<'source, S> {
    pub fn new(s: &'source S) -> Self {
        Self { s, offset: 0 }
    }

    pub fn read_next_exact(&mut self, buf: &mut [u8]) -> Result<(), InkError> {
        self.s.read_exact_at(self.offset, buf)?;
        self.offset += buf.len() as u64;
        Ok(())
    }

    pub fn read_next_u32(&mut self) -> Result<u32, InkError> {
        let mut buf = [0u8; 4];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u32, buf))
    }

    pub fn read_next_u16(&mut self) -> Result<u16, InkError> {
        let mut buf = [0u8; 2];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u16, buf))
    }

    pub fn read_next_u8(&mut self) -> Result<u8, InkError> {
        let mut buf = [0u8; 1];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u8, buf))
    }

    pub fn read_next_array<const N: usize>(&mut self) -> Result<[u8; N], InkError> {
        let mut buf = [0u8; N];
        self.read_next_exact(&mut buf)?;
        Ok(buf)
    }
}
#[macro_export]
macro_rules! to_int {
    (u8, $x:expr) => {{ u8::from_be_bytes($x) }};
    (u16, $x:expr) => {{ u16::from_be_bytes($x) }};
    (u32, $x:expr) => {{ u32::from_be_bytes($x) }};
    (u64, $x:expr) => {{ u64::from_be_bytes($x) }};
}

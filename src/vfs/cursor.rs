use super::file::InkFile;
use crate::InkError;
#[expect(unused)]
use crate::MemCursor;
use crate::to_int;

/// A cursor that walks the bytes of an [`InkFile`] from offset zero.
///
/// It remembers where it stopped, so consecutive reads move forward on their
/// own, and it decodes the big-endian integers the database header and the
/// pages are written in. It is used to validate the database header and rarely
/// outside of it: any code working on bytes it already holds should reach for
/// [`MemCursor`] instead, which offers the same reads plus seeking, peeking and
/// varints.
pub(crate) struct FileCursor<'source, S: ?Sized> {
    s: &'source S,
    offset: u64,
}

impl<'source, S: ?Sized + InkFile> FileCursor<'source, S> {
    /// Start at the beginning of the file.
    pub fn new(s: &'source S) -> Self {
        Self { s, offset: 0 }
    }

    /// Fill `buf` with the next `buf.len()` bytes and move past them.
    pub fn read_next_exact(&mut self, buf: &mut [u8]) -> Result<(), InkError> {
        self.s.read_exact_at(self.offset, buf)?;
        self.offset += buf.len() as u64;
        Ok(())
    }

    /// Read the next four bytes as a big-endian `u32`.
    pub fn read_next_u32(&mut self) -> Result<u32, InkError> {
        let mut buf = [0u8; 4];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u32, buf))
    }

    /// Read the next two bytes as a big-endian `u16`.
    pub fn read_next_u16(&mut self) -> Result<u16, InkError> {
        let mut buf = [0u8; 2];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u16, buf))
    }

    /// Read the next byte.
    pub fn read_next_u8(&mut self) -> Result<u8, InkError> {
        let mut buf = [0u8; 1];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u8, buf))
    }

    /// Read the next `N` bytes as a fixed size array.
    pub fn read_next_array<const N: usize>(&mut self) -> Result<[u8; N], InkError> {
        let mut buf = [0u8; N];
        self.read_next_exact(&mut buf)?;
        Ok(buf)
    }
}
/// Decode a big-endian byte array into the integer named in the first position.
#[macro_export]
macro_rules! to_int {
    (u8, $x:expr) => {{ u8::from_be_bytes($x) }};
    (u16, $x:expr) => {{ u16::from_be_bytes($x) }};
    (u32, $x:expr) => {{ u32::from_be_bytes($x) }};
    (u64, $x:expr) => {{ u64::from_be_bytes($x) }};
}

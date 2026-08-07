use crate::bytes;
use crate::decode_varint;
use crate::to_int;
use crate::util::sqlite_assert_one;
use crate::util::sqlite_assert_with_corrupt_err;
use crate::SqliteError;

#[derive(Debug)]
pub struct SqliteCursor<'a> {
    bytes: &'a [u8],
    offset: u64,
}
impl<'a> SqliteCursor<'a> {
    pub fn new<S: AsRef<[u8]> + ?Sized>(bytes: &'a S) -> Self {
        Self {
            bytes: bytes.as_ref(),
            offset: 0,
        }
    }

    pub fn with_offset<S: AsRef<[u8]> + ?Sized>(
        bytes: &'a S,
        offset: u64,
    ) -> Result<Self, SqliteError> {
        // todo: i may remove this later
        sqlite_assert_with_corrupt_err(
            offset as usize <= bytes.as_ref().len(),
            "offset is bigger than the bytes length",
        )?;
        Ok(Self {
            bytes: bytes.as_ref(),
            offset,
        })
    }
    pub fn set_offset(&mut self, offset: u64) {
        self.offset = offset;
    }

    pub fn move_forward_by(&mut self, steps: u64) -> Result<(), SqliteError> {
        self.offset.checked_add(steps).ok_or(SqliteError::OverFlow(
            "Overflow while trying to move the cursor forward".into(),
        ))?;
        Ok(())
    }

    pub fn move_backward_by(&mut self, steps: u64) -> Result<(), SqliteError> {
        self.offset.checked_sub(steps).ok_or(SqliteError::OverFlow(
            "Overflow while trying to move the cursor backward".into(),
        ))?;
        Ok(())
    }
    pub fn stream_pos(&self) -> u64 {
        self.offset
    }
    pub fn reset(&mut self) {
        self.offset = 0;
    }

    pub fn read_next_exact<B: AsMut<[u8]> + ?Sized>(
        &mut self,
        buf: &mut B,
    ) -> Result<(), SqliteError> {
        let buf = buf.as_mut();
        sqlite_assert_with_corrupt_err(
            self.offset as usize + buf.len() <= self.bytes.len(),
            "reading this buffer will cause an overflow",
        )?;
        let offset = self.offset as usize;
        let slice = &self.bytes[offset..offset + buf.len()];
        buf.copy_from_slice(slice);
        // TODO: MAY THIS CAUSE AN OVERFLOW IN THE PREVIOUS ASSERT?
        self.offset += buf.len() as u64;
        Ok(())
    }
    pub fn read_next_u32(&mut self) -> Result<u32, SqliteError> {
        let mut buf = [0u8; 4];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u32, buf))
    }

    pub fn read_next_u16(&mut self) -> Result<u16, SqliteError> {
        let mut buf = [0u8; 2];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u16, buf))
    }

    pub fn read_next_u8(&mut self) -> Result<u8, SqliteError> {
        let mut buf = [0u8; 1];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u8, buf))
    }
    pub fn read_next_array<const N: usize>(&mut self) -> Result<[u8; N], SqliteError> {
        let mut buf = [0u8; N];
        self.read_next_exact(&mut buf)?;
        Ok(buf)
    }

    // read without moving the cursor
    pub fn read_at<B: AsMut<[u8]> + ?Sized>(
        &mut self,
        buf: &mut B,
        offset: u64,
    ) -> Result<(), SqliteError> {
        let buf = buf.as_mut();
        sqlite_assert_with_corrupt_err(
            offset as usize + buf.len() <= self.bytes.len(),
            "reading this buffer will cause an overflow",
        )?;
        let offset = offset as usize;
        let slice = &self.bytes[offset..offset + buf.len()];
        buf.copy_from_slice(slice);
        Ok(())
    }

    pub fn read_u32_at(&mut self, offset: u64) -> Result<u32, SqliteError> {
        let mut buf = [0u8; 4];
        self.read_at(&mut buf, offset)?;
        Ok(to_int!(u32, buf))
    }

    pub fn read_u16_at(&mut self, offset: u64) -> Result<u16, SqliteError> {
        let mut buf = [0u8; 2];
        self.read_at(&mut buf, offset)?;
        Ok(to_int!(u16, buf))
    }

    pub fn read_u8_at(&mut self, offset: u64) -> Result<u8, SqliteError> {
        let mut buf = [0u8; 1];
        self.read_at(&mut buf, offset)?;
        Ok(to_int!(u8, buf))
    }
    pub fn read_array_at<const N: usize>(&mut self) -> Result<[u8; N], SqliteError> {
        let mut buf = [0u8; N];
        self.read_next_exact(&mut buf)?;
        Ok(buf)
    }
    pub fn read_varint_at(
        &self,
        offset: u64,
        usable_size: usize,
    ) -> Result<(u64, usize), SqliteError> {
        let remaining_bytes = self.remaining_varint_bytes(offset, usable_size)?;
        let offset = offset as usize;
        let bytes = &self.bytes[offset..offset + remaining_bytes];
        decode_varint(bytes).ok_or(SqliteError::InvalidVarint)
    }
    pub fn read_next_varint(&mut self, usable_size: usize) -> Result<(u64, usize), SqliteError> {
        let remaining_bytes = self.remaining_varint_bytes(self.offset, usable_size)?;
        let offset = self.offset as usize;
        let bytes = &self.bytes[offset..offset + remaining_bytes];
        let (int, consumed) = decode_varint(bytes).ok_or(SqliteError::InvalidVarint)?;
        self.offset += consumed as u64;
        Ok((int, consumed))
    }

    fn remaining_varint_bytes(
        &self,
        offset: u64,
        usable_size: usize,
    ) -> Result<usize, SqliteError> {
        let offset = offset as usize;
        let remaining = usable_size
            .checked_sub(offset)
            .ok_or(SqliteError::InvalidVarint)?;

        Ok(remaining.min(9))
    }
}

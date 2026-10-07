use crate::InkError;
use crate::to_int;
use crate::util::assert_with_corrupt_err;
use crate::util::assert_with_runtime_err;
use crate::varint::decode_varint;

/// A cursor that walks a slice of bytes.
///
/// It holds one offset, counted from the start of the bytes, and moves that
/// offset along as values are read. Every read checks first, so running off the
/// end comes back as an error rather than a panic.
#[derive(Debug)]
pub struct MemCursor<'a> {
    bytes: &'a [u8],
    offset: u64,
}
impl<'a> MemCursor<'a> {
    pub fn new<S: AsRef<[u8]> + ?Sized>(bytes: &'a S) -> Self {
        Self {
            bytes: bytes.as_ref(),
            offset: 0,
        }
    }

    pub fn with_offset<S: AsRef<[u8]> + ?Sized>(
        bytes: &'a S,
        offset: u64,
    ) -> Result<Self, InkError> {
        assert_with_corrupt_err(offset as usize <= bytes.as_ref().len(), || {
            format!(
                "The given offset ({}) is bigger than the bytes length ({})",
                offset,
                bytes.as_ref().len()
            )
        })?;
        Ok(Self {
            bytes: bytes.as_ref(),
            offset,
        })
    }
    /// A copy of this cursor, at the same offset.
    pub fn clone_cursor(&self) -> Self {
        Self {
            bytes: self.bytes,
            offset: self.offset,
        }
    }
    /// A copy of this cursor pointed at another offset, which has to be within the bytes.
    pub fn clone_with_offset(&self, offset: u64) -> Result<Self, InkError> {
        Self::with_offset(self.bytes, offset)
    }
    /// Point the cursor at an offset directly. Nothing is checked here, so an
    /// offset past the end only shows up when the next read happens.
    pub fn set_offset(&mut self, offset: u64) {
        self.offset = offset;
    }

    /// Step the cursor forward.
    ///
    /// # Errors
    /// When adding the step overflows the counter.
    pub fn move_forward_by(&mut self, steps: u64) -> Result<(), InkError> {
        self.offset = self.offset.checked_add(steps).ok_or(InkError::Overflow(
            "Overflow while trying to move the cursor forward".into(),
        ))?;
        Ok(())
    }

    /// Step the cursor back.
    ///
    /// # Errors
    /// When the step would take the offset below zero.
    pub fn move_backward_by(&mut self, steps: u64) -> Result<(), InkError> {
        self.offset = self.offset.checked_sub(steps).ok_or(InkError::Overflow(
            "Overflow while trying to move the cursor backward".into(),
        ))?;
        Ok(())
    }
    /// Where the cursor is now.
    pub fn stream_pos(&self) -> u64 {
        self.offset
    }
    /// Put the cursor back at the start.
    pub fn reset(&mut self) {
        self.offset = 0;
    }

    /// Fill a buffer with the next bytes and step past them.
    pub fn read_next_exact<B: AsMut<[u8]> + ?Sized>(
        &mut self,
        buf: &mut B,
    ) -> Result<(), InkError> {
        let buf = buf.as_mut();
        assert_with_runtime_err(self.offset as usize + buf.len() <= self.bytes.len(), || {
            format!(
                "Reading this buffer will cause an overflow\noffset:{} buffer len: {}, bytes len: {}",
                self.offset,
                buf.len(),
                self.bytes.len()
            )
        })?;
        let offset = self.offset as usize;
        let slice = &self.bytes[offset..offset + buf.len()];
        buf.copy_from_slice(slice);
        self.offset += buf.len() as u64;
        Ok(())
    }
    /// Read the next four bytes as a big endian number.
    pub fn read_next_u32(&mut self) -> Result<u32, InkError> {
        let mut buf = [0u8; 4];
        self.read_next_exact(&mut buf)?;
        Ok(to_int!(u32, buf))
    }

    /// Read the next two bytes as a big endian number.
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
    /// Read the next N bytes into an array.
    pub fn read_next_array<const N: usize>(&mut self) -> Result<[u8; N], InkError> {
        let mut buf = [0u8; N];
        self.read_next_exact(&mut buf)?;
        Ok(buf)
    }

    /// Borrow the next `ahead_by` bytes and step past them.
    ///
    /// # Errors
    /// When the read would run off the end, reported as a corrupt page.
    pub fn read_to(&mut self, ahead_by: u64) -> Result<&'a [u8], InkError> {
        let ahead_by = ahead_by as usize;
        assert_with_corrupt_err(ahead_by <= self.bytes.len(), || {
            format!(
                "Cursor advanced past the end of the buffer: attempted offset {} exceeds buffer length {}",
                ahead_by,
                self.bytes.len()
            )
        })?;
        let offset = self.offset as usize;
        let buf = &self.bytes[offset..offset + ahead_by];
        self.offset += ahead_by as u64;
        Ok(buf)
    }

    /// Borrow the next `ahead_by` bytes without moving the cursor.
    ///
    /// # Errors
    /// When the peek would run off the end, reported as a corrupt page.
    pub fn peek_to(&self, ahead_by: u64) -> Result<&[u8], InkError> {
        let ahead_by = ahead_by as usize;
        assert_with_corrupt_err(ahead_by <= self.bytes.len(), || {
            format!(
                "Cursor peeked past the end of the buffer: attempted offset {} exceeds buffer length {}",
                ahead_by,
                self.bytes.len()
            )
        })?;
        let offset = self.offset as usize;
        let buf = &self.bytes[offset..offset + ahead_by];
        Ok(buf)
    }

    /// Fill a buffer from an offset, leaving the cursor where it is.
    pub fn read_at<B: AsMut<[u8]> + ?Sized>(
        &mut self,
        buf: &mut B,
        offset: u64,
    ) -> Result<(), InkError> {
        let buf = buf.as_mut();
        assert_with_runtime_err(self.offset as usize + buf.len() <= self.bytes.len(), || {
            format!(
                "Reading this buffer will cause an overflow\noffset:{} buffer len: {}, bytes len: {}",
                self.offset,
                buf.len(),
                self.bytes.len()
            )
        })?;
        let offset = offset as usize;
        let slice = &self.bytes[offset..offset + buf.len()];
        buf.copy_from_slice(slice);
        Ok(())
    }

    /// Read four bytes at an offset as a big endian number.
    pub fn read_u32_at(&mut self, offset: u64) -> Result<u32, InkError> {
        let mut buf = [0u8; 4];
        self.read_at(&mut buf, offset)?;
        Ok(to_int!(u32, buf))
    }

    /// Read two bytes at an offset as a big endian number.
    pub fn read_u16_at(&mut self, offset: u64) -> Result<u16, InkError> {
        let mut buf = [0u8; 2];
        self.read_at(&mut buf, offset)?;
        Ok(to_int!(u16, buf))
    }

    /// Read one byte at an offset.
    pub fn read_u8_at(&mut self, offset: u64) -> Result<u8, InkError> {
        let mut buf = [0u8; 1];
        self.read_at(&mut buf, offset)?;
        Ok(to_int!(u8, buf))
    }
    /// Read the next N bytes into an array, the same as `read_next_array`.
    pub fn read_array_at<const N: usize>(&mut self) -> Result<[u8; N], InkError> {
        let mut buf = [0u8; N];
        self.read_next_exact(&mut buf)?;
        Ok(buf)
    }
    /// Read a varint at an offset, leaving the cursor where it is. `usable_size`
    /// caps how many bytes are looked at.
    pub fn read_varint_at(
        &self,
        offset: u64,
        usable_size: usize,
    ) -> Result<(u64, usize), InkError> {
        let remaining_bytes = self.remaining_varint_bytes(offset, usable_size)?;
        let offset = offset as usize;
        let bytes = &self.bytes[offset..offset + remaining_bytes];
        decode_varint(bytes).ok_or(InkError::InvalidVarint)
    }
    /// Read a varint at the cursor and step past it. The value and the number of
    /// bytes it took both come back.
    pub fn read_next_varint(&mut self, usable_size: usize) -> Result<(u64, usize), InkError> {
        let remaining_bytes = self.remaining_varint_bytes(self.offset, usable_size)?;
        let offset = self.offset as usize;
        let bytes = &self.bytes[offset..offset + remaining_bytes];
        let (int, consumed) = decode_varint(bytes).ok_or(InkError::InvalidVarint)?;
        self.offset += consumed as u64;
        Ok((int, consumed))
    }

    /// How many bytes to hand the varint decoder: whatever is left of the usable
    /// area, capped at nine, since a varint is never longer than that.
    fn remaining_varint_bytes(&self, offset: u64, usable_size: usize) -> Result<usize, InkError> {
        let offset = offset as usize;
        let remaining = usable_size
            .checked_sub(offset)
            .ok_or(InkError::InvalidVarint)?;

        Ok(remaining.min(9))
    }
}

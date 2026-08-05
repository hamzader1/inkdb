use crate::bytes;
use crate::decode_varint;
use crate::to_int;
use crate::SqliteError;

pub struct PageCursor<'a> {
    bytes: &'a [u8],
    offset: u64,
}
impl<'a> PageCursor<'a> {
    pub fn new<S: AsRef<[u8]> + ?Sized>(bytes: &'a S) -> Self {
        Self {
            bytes: bytes.as_ref(),
            offset: 0,
        }
    }

    pub fn new_offset<S: AsRef<[u8]> + ?Sized>(bytes: &'a S, offset: u64) -> Self {
        Self {
            bytes: bytes.as_ref(),
            offset,
        }
    }
    pub fn set_offset(&mut self, offset: u64) {
        self.offset = offset
    }
    fn current_pos(&self) -> u64 {
        self.offset
    }
    fn reset(&mut self) {
        self.offset = 0;
    }

    pub fn read_next_exact<B: AsMut<[u8]> + ?Sized>(&mut self, buf: &mut B) {
        let buf = buf.as_mut();
        assert!(
            self.offset as usize + buf.len() <= self.bytes.len(),
            "reading this buffer will cause an overflow"
        );
        let offset = self.offset as usize;
        let slice = &self.bytes[offset..offset + buf.len()];
        buf.copy_from_slice(slice);
        // TODO: MAY THIS CAUSE AN OVERFLOW?
        self.offset += buf.len() as u64;
    }
    pub fn read_next_u32(&mut self) -> u32 {
        let mut buf = [0u8; 4];
        self.read_next_exact(&mut buf);
        to_int!(u32, buf)
    }

    pub fn read_next_u16(&mut self) -> u16 {
        let mut buf = [0u8; 2];
        self.read_next_exact(&mut buf);
        to_int!(u16, buf)
    }

    pub fn read_next_u8(&mut self) -> u8 {
        let mut buf = [0u8; 1];
        self.read_next_exact(&mut buf);
        to_int!(u8, buf)
    }
    pub fn read_next_arrary<const N: usize>(&mut self) -> [u8; N] {
        let mut buf = [0u8; N];
        self.read_next_exact(&mut buf);
        buf
    }

    pub fn read_at<B: AsMut<[u8]> + ?Sized>(&mut self, buf: &mut B, offset: u64) {
        let buf = buf.as_mut();
        assert!(
            offset as usize + buf.len() <= self.bytes.len(),
            "reading this buffer will cause an overflow"
        );
        let offset = offset as usize;
        let slice = &self.bytes[offset..offset + buf.len()];
        buf.copy_from_slice(slice);
        // TODO 2: MAY THIS CAUSE AN OVERFLOW?
        self.offset += buf.len() as u64;
    }

    pub fn read_u32_at(&mut self, offset: u64) -> u32 {
        let mut buf = [0u8; 4];
        self.read_at(&mut buf, offset);
        to_int!(u32, buf)
    }

    pub fn read_u16(&mut self, offset: u64) -> u16 {
        let mut buf = [0u8; 2];
        self.read_at(&mut buf, offset);
        to_int!(u16, buf)
    }

    pub fn read_u8(&mut self, offset: u64) -> u8 {
        let mut buf = [0u8; 1];
        self.read_at(&mut buf, offset);
        to_int!(u8, buf)
    }
    pub fn read_arrary<const N: usize>(&mut self) -> [u8; N] {
        let mut buf = [0u8; N];
        self.read_next_exact(&mut buf);
        buf
    }
    pub fn read_varint_next(&self, usable_size: usize) -> (u64, usize) {
        let remaining_bytes = self.remaining_varint_bytes(self.offset, usable_size);
        let bytes = &(&[0u8; 9])[0..remaining_bytes];
        decode_varint(bytes).expect("Failed to read decode varint")
    }

    pub fn read_varint_at(&self, offset: u64, usable_size: usize) -> (u64, usize) {
        let remaining_bytes = self.remaining_varint_bytes(offset, usable_size);
        let bytes = &(&[0u8; 9])[0..remaining_bytes];
        decode_varint(bytes).expect("Failed to read decode varint")
    }
    fn remaining_varint_bytes(&self, offset: u64, usable_size: usize) -> usize {
        let offset = offset as usize;
        let remaining = usable_size
            .checked_sub(offset)
            .expect(&*SqliteError::InvalidVarint.to_string());

        remaining.min(9)
    }

}

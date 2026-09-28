#![expect(unused)]
use crate::{InkError, to_int};
use std::io::Read;

pub fn read_u8<R: Read>(r: &mut R) -> Result<u8, InkError> {
    let mut buf = [0; 1];
    r.read_exact(&mut buf)
        .map_err(|_| InkError::DatabaseCorrupted)?;
    Ok(to_int!(u8, buf))
}

pub fn read_u16_be<R: Read>(r: &mut R) -> Result<u16, InkError> {
    let mut buf = [0; 2];
    r.read_exact(&mut buf)
        .map_err(|_| InkError::DatabaseCorrupted)?;
    Ok(to_int!(u16, buf))
}

pub fn read_u32_be<R: Read>(r: &mut R) -> Result<u32, InkError> {
    let mut buf = [0; 4];
    r.read_exact(&mut buf)
        .map_err(|_| InkError::DatabaseCorrupted)?;
    Ok(to_int!(u32, buf))
}

pub fn read_array<const N: usize, R: Read>(r: &mut R) -> Result<[u8; N], InkError> {
    let mut buf = [0; N];
    r.read_exact(&mut buf)
        .map_err(|_| InkError::DatabaseCorrupted)?;
    Ok(buf)
}

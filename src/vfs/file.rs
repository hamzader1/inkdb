use crate::InkError;
use std::path::PathBuf;

#[allow(clippy::len_without_is_empty)]
pub trait InkFile: std::fmt::Debug {
    fn name(&self) -> &str;

    fn path(&self) -> PathBuf;

    fn len(&self) -> Result<u64, InkError>;

    fn read_exact_at(&self, offset: u64, buff: &mut [u8]) -> Result<(), InkError>;

    fn write_all_at(&self, offset: u64, buff: &[u8]) -> Result<(), InkError>;

    fn set_len(&self, len: usize) -> Result<(), InkError>;

    fn sync(&self) -> Result<(), InkError>;
}

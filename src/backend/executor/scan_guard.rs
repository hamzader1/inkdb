use crate::{
    SqliteResult,
    pager::pager::Pager,
    storage::btree::{BTreeCursor, RestorePosition},
    vfs::Vfs,
};

pub trait ScanGuard<V: Vfs>: std::fmt::Debug {
    fn restore(&mut self, pager: &mut Pager<V>, cursor: &mut BTreeCursor<V>) -> SqliteResult<()> {
        Ok(())
    }
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<V>,
        cursor: &mut BTreeCursor<V>,
    ) -> SqliteResult<()> {
        Ok(())
    }
    fn scan_type(&self) -> &'static str;
}

// Unsafe means the row may vanish under us through Delete. When the
// saved row is still there we step over it, when it is gone the
// restore already parks on its successor. Either way the cursor is
// valid here or the scan is over.
#[derive(Debug)]
pub struct UnsafeScan;
impl<V: Vfs> ScanGuard<V> for UnsafeScan {
    fn restore(&mut self, pager: &mut Pager<V>, cursor: &mut BTreeCursor<V>) -> SqliteResult<()> {
        match cursor.restore_position(pager)? {
            RestorePosition::Exact => cursor.next(pager)?,
            RestorePosition::Next | RestorePosition::Empty => {}
        };
        Ok(())
    }
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<V>,
        cursor: &mut BTreeCursor<V>,
    ) -> SqliteResult<()> {
        cursor.save_position(pager)
    }
    fn scan_type(&self) -> &'static str {
        "UnsafeScan"
    }
}

#[derive(Debug)]
pub struct SafeScan;
impl<V: Vfs> ScanGuard<V> for SafeScan {
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<V>,
        cursor: &mut BTreeCursor<V>,
    ) -> SqliteResult<()> {
        cursor.next(pager)
    }
    fn scan_type(&self) -> &'static str {
        "SafeScan"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    Stable,
    Volatile,
}

impl ScanMode {
    pub fn guard<V: Vfs>(self) -> Box<dyn ScanGuard<V>> {
        match self {
            Self::Stable => Box::new(SafeScan),
            Self::Volatile => Box::new(UnsafeScan),
        }
    }
}

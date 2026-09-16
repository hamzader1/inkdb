use crate::{
    SqliteResult,
    pager::pager::Pager,
    storage::btree::{BTreeCursor, RestorePosition},
    vfs::file::SqliteFile,
};

pub trait ScanGuard<F: SqliteFile>: std::fmt::Debug {
    fn restore(&mut self, pager: &mut Pager<F>, cursor: &mut BTreeCursor<F>) -> SqliteResult<()> {
        Ok(())
    }
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<F>,
        cursor: &mut BTreeCursor<F>,
    ) -> SqliteResult<()> {
        Ok(())
    }
    fn scan_type(&self) -> String;
}

// Unsafe means the row may vanish under us through Delete. When the
// saved row is still there we step over it, when it is gone the
// restore already parks on its successor. Either way the cursor is
// valid here or the scan is over.
#[derive(Debug)]
pub struct UnsafeScan;
impl<F: SqliteFile> ScanGuard<F> for UnsafeScan {
    fn restore(&mut self, pager: &mut Pager<F>, cursor: &mut BTreeCursor<F>) -> SqliteResult<()> {
        match cursor.restore_position(pager)? {
            RestorePosition::Exact => cursor.next(pager)?,
            RestorePosition::Next | RestorePosition::Empty => {}
        };
        Ok(())
    }
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<F>,
        cursor: &mut BTreeCursor<F>,
    ) -> SqliteResult<()> {
        cursor.save_position(pager)
    }
    fn scan_type(&self) -> String {
        "UnsafeScan".into()
    }
}

#[derive(Debug)]
pub struct SafeScan;
impl<F: SqliteFile> ScanGuard<F> for SafeScan {
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<F>,
        cursor: &mut BTreeCursor<F>,
    ) -> SqliteResult<()> {
        cursor.next(pager)
    }
    fn scan_type(&self) -> String {
        "SafeScan".into()
    }
}

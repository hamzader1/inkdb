

pub mod cursor;
pub mod tree;
pub mod delete;
pub mod insert;
pub mod kind;
pub mod policy;
pub mod rebalance;
pub mod typed_mut;

pub use cursor::{BTreeCursor, CursorState, Path, RestorePosition, SeekResult};
pub use insert::BTree;
pub use kind::{IndexInterior, IndexLeaf, TableInterior, TableLeaf};

use std::cmp::Ordering;

use crate::SqliteError;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::page::PageRef;

pub type CellIndex = u16;

pub fn page_as_ref_with_pager<'b, V: crate::vfs::Vfs>(
    page_no: PageNo,
    guard: &'b PageGuard,
    pager: &Pager<V>,
) -> Result<PageRef<'b>, SqliteError> {
    PageRef::new(
        page_no,
        pager.page_size(),
        pager.usable_size(),
        guard.bytes(),
    )
}

pub fn page_as_mut_with_pager<'b, V: crate::vfs::Vfs>(
    page_no: PageNo,
    guard: &'b mut PageGuard,
    pager: &Pager<V>,
) -> Result<crate::storage::page::PageMut<'b>, SqliteError> {
    let bytes = guard.bytes_as_mut().ok_or_else(|| {
        SqliteError::Internal("page_as_mut_with_pager: guard is not a mutable borrow".into())
    })?;
    crate::storage::page::PageMut::new(page_no, pager.page_size(), pager.usable_size(), bytes)
}

pub(crate) fn compare_index_entry(
    entry: &[Value],
    target: &Value,
) -> Result<(Ordering, bool), SqliteError> {
    let keys = match target {
        Value::Tuple(cols) => cols,
        _ => {
            return Err(SqliteError::Internal(
                "index seek target must be a tuple of key columns".into(),
            ));
        }
    };
    if keys.len() > entry.len() {
        return Err(SqliteError::Internal(format!(
            "index seek target has {} columns but entries hold {}",
            keys.len(),
            entry.len()
        )));
    }
    for (stored, wanted) in entry.iter().zip(keys.iter()) {
        if stored == wanted {
            continue;
        }
        if stored > wanted {
            return Ok((Ordering::Greater, false));
        }
        return Ok((Ordering::Less, false));
    }
    Ok((Ordering::Equal, keys.len() == entry.len()))
}



pub mod cursor;
pub mod delete;
pub mod insert;
pub mod kind;
pub mod ops;
pub mod rebalance;
pub mod tree;
pub mod typed_mut;

pub(crate) use cursor::RestorePosition;
pub use cursor::{BTreeCursor, CursorState, SeekResult};
pub(crate) use kind::IndexLeaf;
pub use kind::TableLeaf;

use std::cmp::Ordering;

use crate::InkError;
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::page::PageRef;

pub(crate) use self::tree::BTree;

/// The slot a cell takes in a page cell pointer array.
pub type CellIndex = u16;

/// A read-only page view over a guard bytes, sized from the header values the
/// pager already has.
pub fn page_as_ref_with_pager<'b, V: crate::vfs::Vfs>(
    page_no: PageNo,
    guard: &'b PageGuard,
    pager: &Pager<V>,
) -> Result<PageRef<'b>, InkError> {
    PageRef::new(
        page_no,
        pager.page_size(),
        pager.usable_size(),
        pager.header_len(),
        guard.bytes(),
    )
}

/// The same view over bytes borrowed for writing.
pub(crate) fn page_as_mut_with_pager<'b, V: crate::vfs::Vfs>(
    page_no: PageNo,
    guard: &'b mut PageGuard,
    pager: &Pager<V>,
) -> Result<crate::storage::page::PageMut<'b>, InkError> {
    let bytes = guard.bytes_as_mut().ok_or(InkError::Internal(
        "page_as_mut_with_pager: guard is not a mutable borrow",
    ))?;
    crate::storage::page::PageMut::new(
        page_no,
        pager.page_size(),
        pager.usable_size(),
        pager.header_len(),
        bytes,
    )
}

/// Compare a stored index entry with a seek key.
///
/// Only as many columns as the key holds are looked at, which is what lets a
/// seek on a subset of an index columns find its place. The second value says
/// whether the whole entry was matched, so a full hit can be told apart from
/// one that only matched the leading columns.
pub(crate) fn compare_index_entry(
    entry: &[Value],
    target: &Value,
) -> Result<(Ordering, bool), InkError> {
    let keys = match target {
        Value::Tuple(cols) => cols,
        _ => {
            return Err(InkError::Internal(
                "index seek target must be a tuple of key columns",
            ));
        }
    };
    if keys.len() > entry.len() {
        return Err(InkError::InternalFmt(format!(
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

use crate::InkResult;
use crate::errors::InkError;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::cell::Encode;
use crate::vfs::Vfs;

use super::cursor::{IndexSearchResult, search_indexes, search_row_ids};
use super::kind::{IndexInterior, IndexLeaf, PageKind, TableInterior, TableLeaf, TypedPage};
use crate::storage::btree::CellIndex;
use crate::storage::cell::{IndexInteriorCell, TableInteriorCell, TableLeafCell};

/// The key that separates two children of an interior page.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Divider {
    /// A row id, which is how a table interior page divides its children.
    RowId(u64),
    /// A whole index entry. The bytes are the entry without its child pointer,
    /// kept alongside the parsed key so it can be written back into a parent
    /// page without being encoded again.
    Entry {
        cell: Box<[u8]>,
        key: Value<'static>,
    },
}

impl Divider {
    /// The interior cell that puts this divider in front of a child.
    pub fn cell_for(&self, child: PageNo) -> Vec<u8> {
        match self {
            Divider::RowId(row_id) => Encode::encode_table_interior_cell(child, *row_id),
            Divider::Entry { cell, .. } => Encode::encode_index_interior_cell(child, cell),
        }
    }

    /// The key the divider sorts under.
    pub fn key(&self) -> Value<'static> {
        match self {
            Divider::RowId(row_id) => Value::Integer(*row_id as i64),
            Divider::Entry { key, .. } => key.clone(),
        }
    }
}

/// What happened to the cell a split promoted out of the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Consumed {
    /// Nothing was taken: the divider is only the row id that bounds the left
    /// half, and the cell it came from stays on the page.
    None,
    /// The last cell of the left half was taken, so the left half loses it.
    LastOfLeft,
    /// The first cell of the right half was taken, so the right half loses it.
    /* This will be used soon. */
    #[allow(dead_code)]
    FirstOfRight,
}

/// The divider a split pushes up to the parent, with what it did to the cells
/// on the way.
pub(crate) struct Promotion {
    /// The key separating the two halves.
    pub divider: Divider,
    /// Which cell, if any, was taken from a half to be the divider.
    pub consumed: Consumed,
    /// The right-most child the left half keeps, when the divider was an
    /// interior cell and carried that pointer with it.
    pub left_rmp: Option<PageNo>,
}

/// The two halves a page was split into.
pub(crate) struct Split {
    /// The page the left half stayed in.
    pub left_page: PageNo,
    /// The fresh page the right half went to.
    pub right_page: PageNo,
    /// The divider that separates the halves.
    pub divider: Divider,
    /// The divider of the last cell on the right half, which is what the parent
    /// already has a cell for.
    pub right_bound: Divider,
}

/// Where a new divider belongs among a parent page existing cells.
pub(crate) enum ParentSlot {
    /// The child that was split is the right-most one, so the divider goes in a
    /// new cell and the right-most pointer moves along.
    RightMost,
    /// The parent already has a cell for the child that was split, so that cell
    /// takes over one of the halves and a new cell is added for the other.
    Existing {
        cell: Box<[u8]>,
        key: Value<'static>,
    },
}

/// What code that works over any page kind needs to do with cells.
///
/// The two things that differ between kinds, how a cell key is compared and what
/// a promoted cell turns into, are the whole of this trait.
pub(crate) trait CellOps: PageKind + Sized {
    /// The slot a key belongs in: the cell it matches, or the one just after
    /// where it would have gone.
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> InkResult<CellIndex>;

    /// The key cell i sorts under.
    fn key_of<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> InkResult<Value<'static>>;

    /// Cell i as a divider, for when a page needs one to push up.
    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> InkResult<Divider>;

    /// Turn the cell at the split point into the divider that goes up.
    ///
    /// An index entry is stored once in the whole tree, so the cell at the split
    /// point leaves the page and becomes the divider, and so does a table
    /// interior cell, since a row id on an interior page is not a row. The left
    /// child of an interior cell goes up with it, as the right-most pointer of
    /// the left half. A table leaf keeps its row, since a divider over a table
    /// leaf is only the row id that bounds it.
    fn promote<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        split_at: CellIndex,
        pager: &mut Pager<V>,
    ) -> InkResult<Promotion>;
}

/// What an interior page has to answer, on top of what every page does.
pub(crate) trait InteriorOps: RebalanceOps {
    /// The child page cell i points at.
    fn child_of<B: AsRef<[u8]>>(page: &TypedPage<B, Self>, i: CellIndex) -> InkResult<PageNo>;

    /// The divider an existing parent cell becomes once its child has been
    /// split. On an index page only the key matters, so the old cell is reused;
    /// on a table page the cell bounds the left half, and the right half needs
    /// the new bound.
    fn right_divider(old_cell: &[u8], old_key: Value<'static>, right_bound: Divider) -> Divider;
}

/// A leaf page, together with the kind of interior page that holds its subtree.
pub(crate) trait LeafKind: RebalanceOps {
    /// The interior page kind whose cells point at this leaf kind.
    type Parent: InteriorOps;
}

/// How two pages share their cells out again when they are too full to merge.
pub(crate) struct Redistribute {
    /// The cell that replaces the old divider in the parent.
    pub parent_cell: Vec<u8>,
    /// Whether the cell promoted to the parent leaves the left half. It does,
    /// except on a table leaf, where the divider is only a row id and the row
    /// stays where it is.
    pub drop_left_last: bool,
    /// The right-most child the left half keeps when an interior cell was
    /// promoted out of it.
    pub left_rmp: Option<PageNo>,
}

/// What a page has to work out when two neighbours are merged or shared out.
pub(crate) trait RebalanceOps: CellOps {
    /// Bring the parent divider down into the run of cells being rebalanced.
    ///
    /// All but a table leaf have something to bring, since the divider would
    /// otherwise be lost: an index page brings the entry down, and a table
    /// interior page brings its row id back along with the right-most child of
    /// the left half. A table leaf answers with nothing, since its divider is
    /// only a row id and every row is still on the leaves.
    fn pull_down(
        sep_cell: &[u8],
        left_rmp: Option<PageNo>,
        usable: usize,
    ) -> InkResult<Option<Vec<u8>>>;

    /// Work out how the run of cells splits across the two pages, and which
    /// cell becomes the parent divider.
    fn redistribute(
        left_share: &[Vec<u8>],
        right_share: &[Vec<u8>],
        left_page: PageNo,
        left_rmp: Option<PageNo>,
        sep_cell: &[u8],
        usable: usize,
    ) -> InkResult<Redistribute>;
}

impl CellOps for TableInterior {
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> InkResult<CellIndex> {
        let row_id = key.cast_int()? as u64;
        let (_, idx) = search_row_ids(page, pager, row_id)?;
        Ok(idx)
    }

    fn key_of<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        _pager: &mut Pager<V>,
    ) -> InkResult<Value<'static>> {
        Ok(Value::Integer(page.cell(i)?.rowid_boundary as i64))
    }

    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        _pager: &mut Pager<V>,
    ) -> InkResult<Divider> {
        Ok(Divider::RowId(page.cell(i)?.rowid_boundary))
    }

    fn promote<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        split_at: CellIndex,
        _pager: &mut Pager<V>,
    ) -> InkResult<Promotion> {
        let i = split_at - 1;
        let cell = page.cell(i)?;
        Ok(Promotion {
            divider: Divider::RowId(cell.rowid_boundary),
            consumed: Consumed::LastOfLeft,
            left_rmp: Some(cell.left_child),
        })
    }
}

impl InteriorOps for TableInterior {
    fn child_of<B: AsRef<[u8]>>(page: &TypedPage<B, Self>, i: CellIndex) -> InkResult<PageNo> {
        Ok(page.cell(i)?.left_child)
    }

    fn right_divider(_old_cell: &[u8], _old_key: Value<'static>, right_bound: Divider) -> Divider {
        right_bound
    }
}

impl CellOps for TableLeaf {
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> InkResult<CellIndex> {
        let row_id = key.cast_int()? as u64;
        let (_, idx) = search_row_ids(page, pager, row_id)?;
        Ok(idx)
    }

    fn key_of<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        _pager: &mut Pager<V>,
    ) -> InkResult<Value<'static>> {
        Ok(Value::Integer(page.cell(i)?.row_id as i64))
    }

    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        _pager: &mut Pager<V>,
    ) -> InkResult<Divider> {
        Ok(Divider::RowId(page.cell(i)?.row_id))
    }

    fn promote<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        split_at: CellIndex,
        _pager: &mut Pager<V>,
    ) -> InkResult<Promotion> {
        Ok(Promotion {
            divider: Divider::RowId(page.cell(split_at - 1)?.row_id),
            consumed: Consumed::None,
            left_rmp: None,
        })
    }
}

impl LeafKind for TableLeaf {
    type Parent = TableInterior;
}

impl CellOps for IndexInterior {
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> InkResult<CellIndex> {
        let idx = match search_indexes(page, pager, key)? {
            IndexSearchResult::Exact(i)
            | IndexSearchResult::EqualPrefix(i)
            | IndexSearchResult::NotFound(i) => i,
        };
        Ok(idx)
    }

    fn key_of<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> InkResult<Value<'static>> {
        page.index_payload_key(i, pager)
    }

    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> InkResult<Divider> {
        // We remove the left child page number by skipping the first
        // 4 bytes
        let cell = page.cell_bytes_as_ref(i)?[4..].to_vec();
        let key = page.index_payload_key(i, pager)?;
        Ok(Divider::Entry {
            cell: cell.into(),
            key,
        })
    }

    fn promote<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        split_at: CellIndex,
        pager: &mut Pager<V>,
    ) -> InkResult<Promotion> {
        let i = split_at - 1;
        let left_child = page.cell(i)?.left_child;
        Ok(Promotion {
            divider: Self::divider_of_cell(page, i, pager)?,
            consumed: Consumed::LastOfLeft,
            left_rmp: Some(left_child),
        })
    }
}

impl InteriorOps for IndexInterior {
    fn child_of<B: AsRef<[u8]>>(page: &TypedPage<B, Self>, i: CellIndex) -> InkResult<PageNo> {
        Ok(page.cell(i)?.left_child)
    }

    fn right_divider(old_cell: &[u8], old_key: Value<'static>, _right_bound: Divider) -> Divider {
        Divider::Entry {
            // We remove the left child page number by skipping the first
            // 4 bytes
            cell: old_cell[4..].to_vec().into(),
            key: old_key,
        }
    }
}

impl CellOps for IndexLeaf {
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> InkResult<CellIndex> {
        let idx = match search_indexes(page, pager, key)? {
            IndexSearchResult::Exact(i)
            | IndexSearchResult::EqualPrefix(i)
            | IndexSearchResult::NotFound(i) => i,
        };
        Ok(idx)
    }

    fn key_of<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> InkResult<Value<'static>> {
        page.index_payload_key(i, pager)
    }

    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> InkResult<Divider> {
        let cell = page.cell_bytes_as_ref(i)?.to_vec();
        let key = page.index_payload_key(i, pager)?;
        Ok(Divider::Entry {
            cell: cell.into(),
            key,
        })
    }

    fn promote<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        split_at: CellIndex,
        pager: &mut Pager<V>,
    ) -> InkResult<Promotion> {
        let i = split_at - 1;
        Ok(Promotion {
            divider: Self::divider_of_cell(page, i, pager)?,
            consumed: Consumed::LastOfLeft,
            left_rmp: None,
        })
    }
}

impl LeafKind for IndexLeaf {
    type Parent = IndexInterior;
}

impl RebalanceOps for TableLeaf {
    fn pull_down(
        _sep_cell: &[u8],
        _left_rmp: Option<PageNo>,
        _usable: usize,
    ) -> InkResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn redistribute(
        left_share: &[Vec<u8>],
        _right_share: &[Vec<u8>],
        left_page: PageNo,
        _left_rmp: Option<PageNo>,
        _sep_cell: &[u8],
        usable: usize,
    ) -> InkResult<Redistribute> {
        let last = left_share
            .last()
            .ok_or(InkError::Internal("redistribute: empty left share"))?;
        let row_id = TableLeafCell::parse(last, usable)?.row_id;
        Ok(Redistribute {
            parent_cell: Encode::encode_table_interior_cell(left_page, row_id),
            drop_left_last: false,
            left_rmp: None,
        })
    }
}

impl RebalanceOps for IndexLeaf {
    fn pull_down(
        sep_cell: &[u8],
        _left_rmp: Option<PageNo>,
        _usable: usize,
    ) -> InkResult<Option<Vec<u8>>> {
        // We remove the left child page number by skipping the first
        // 4 bytes
        Ok(Some(sep_cell[4..].to_vec()))
    }

    fn redistribute(
        left_share: &[Vec<u8>],
        _right_share: &[Vec<u8>],
        left_page: PageNo,
        _left_rmp: Option<PageNo>,
        _sep_cell: &[u8],
        _usable: usize,
    ) -> InkResult<Redistribute> {
        let promoted = left_share
            .last()
            .ok_or(InkError::Internal("redistribute: empty left share"))?;
        Ok(Redistribute {
            parent_cell: Encode::encode_index_interior_cell(left_page, promoted),
            drop_left_last: true,
            left_rmp: None,
        })
    }
}

impl RebalanceOps for TableInterior {
    fn pull_down(
        sep_cell: &[u8],
        left_rmp: Option<PageNo>,
        usable: usize,
    ) -> InkResult<Option<Vec<u8>>> {
        let left_rmp = left_rmp.ok_or(InkError::Internal(
            "pull_down: left interior has no right child",
        ))?;
        let sep = TableInteriorCell::parse(sep_cell, usable)?;
        Ok(Some(Encode::encode_table_interior_cell(
            left_rmp,
            sep.rowid_boundary,
        )))
    }

    fn redistribute(
        left_share: &[Vec<u8>],
        _right_share: &[Vec<u8>],
        left_page: PageNo,
        _left_rmp: Option<PageNo>,
        _sep_cell: &[u8],
        usable: usize,
    ) -> InkResult<Redistribute> {
        let promoted = left_share
            .last()
            .ok_or(InkError::Internal("redistribute: empty left share"))?;
        let promoted_cell = TableInteriorCell::parse(promoted, usable)?;
        Ok(Redistribute {
            parent_cell: Encode::encode_table_interior_cell(
                left_page,
                promoted_cell.rowid_boundary,
            ),
            drop_left_last: true,
            left_rmp: Some(promoted_cell.left_child),
        })
    }
}

impl RebalanceOps for IndexInterior {
    fn pull_down(
        sep_cell: &[u8],
        left_rmp: Option<PageNo>,
        _usable: usize,
    ) -> InkResult<Option<Vec<u8>>> {
        let left_rmp = left_rmp.ok_or(InkError::Internal(
            "pull_down: left interior has no right child",
        ))?;
        // We remove the left child page number by skipping the first
        // 4 bytes
        Ok(Some(Encode::encode_index_interior_cell(
            left_rmp,
            &sep_cell[4..],
        )))
    }

    fn redistribute(
        left_share: &[Vec<u8>],
        _right_share: &[Vec<u8>],
        left_page: PageNo,
        _left_rmp: Option<PageNo>,
        _sep_cell: &[u8],
        usable: usize,
    ) -> InkResult<Redistribute> {
        let promoted = left_share
            .last()
            .ok_or(InkError::Internal("redistribute: empty left share"))?;
        let promoted_cell = IndexInteriorCell::parse(promoted, usable)?;
        Ok(Redistribute {
            // We remove the left child page number by skipping the first
            // 4 bytes.
            parent_cell: Encode::encode_index_interior_cell(left_page, &promoted[4..]),
            drop_left_last: true,
            left_rmp: Some(promoted_cell.left_child),
        })
    }
}

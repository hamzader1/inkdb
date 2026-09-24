use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::cell::Encode;
use crate::vfs::Vfs;

use super::cursor::{IndexSearchResult, search_indexes, search_row_ids};
use super::kind::{IndexInterior, IndexLeaf, PageKind, TableInterior, TableLeaf, TypedPage};
use crate::storage::btree::CellIndex;
use crate::storage::cell::{IndexInteriorCell, TableInteriorCell, TableLeafCell};

#[derive(Debug, Clone, PartialEq)]
pub enum Divider {
    RowId(u64),
    Entry {
        cell: Box<[u8]>,
        key: Value<'static>,
    },
}

impl Divider {
    pub fn cell_for(&self, child: PageNo) -> Vec<u8> {
        match self {
            Divider::RowId(row_id) => Encode::encode_table_interior_cell(child, *row_id),
            Divider::Entry { cell, .. } => Encode::encode_index_interior_cell(child, cell),
        }
    }

    pub fn key(&self) -> Value<'static> {
        match self {
            Divider::RowId(row_id) => Value::Integer(*row_id as i64),
            Divider::Entry { key, .. } => key.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Consumed {
    None,
    LastOfLeft,
    FirstOfRight,
}

pub struct Promotion {
    pub divider: Divider,
    pub consumed: Consumed,
    pub left_rmp: Option<PageNo>,
}

pub struct Split {
    pub left_page: PageNo,
    pub right_page: PageNo,
    pub divider: Divider,
    pub right_bound: Divider,
}

pub enum ParentSlot {
    RightMost,
    Existing {
        cell: Box<[u8]>,
        key: Value<'static>,
    },
}

pub trait CellPolicy: PageKind + Sized {
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> SqliteResult<CellIndex>;

    fn key_of<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> SqliteResult<Value<'static>>;

    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> SqliteResult<Divider>;

    fn promote<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        split_at: CellIndex,
        pager: &mut Pager<V>,
    ) -> SqliteResult<Promotion>;
}

pub trait InteriorPolicy: RebalancePolicy {
    fn child_of<B: AsRef<[u8]>>(page: &TypedPage<B, Self>, i: CellIndex) -> SqliteResult<PageNo>;

    fn right_divider(old_cell: &[u8], old_key: Value<'static>, right_bound: Divider) -> Divider;
}

pub trait LeafKind: RebalancePolicy {
    type Parent: InteriorPolicy;
}

pub struct Redistribute {
    pub parent_cell: Vec<u8>,
    pub drop_left_last: bool,
    pub left_rmp: Option<PageNo>,
}

pub trait RebalancePolicy: CellPolicy {
    fn pull_down(
        sep_cell: &[u8],
        left_rmp: Option<PageNo>,
        usable: usize,
    ) -> SqliteResult<Option<Vec<u8>>>;

    fn redistribute(
        left_share: &[Vec<u8>],
        right_share: &[Vec<u8>],
        left_page: PageNo,
        left_rmp: Option<PageNo>,
        sep_cell: &[u8],
        usable: usize,
    ) -> SqliteResult<Redistribute>;
}

impl CellPolicy for TableInterior {
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> SqliteResult<CellIndex> {
        let row_id = key.cast_int()? as u64;
        let (_, idx) = search_row_ids(page, pager, row_id)?;
        Ok(idx)
    }

    fn key_of<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        _pager: &mut Pager<V>,
    ) -> SqliteResult<Value<'static>> {
        Ok(Value::Integer(page.cell(i)?.rowid_boundary as i64))
    }

    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        _pager: &mut Pager<V>,
    ) -> SqliteResult<Divider> {
        Ok(Divider::RowId(page.cell(i)?.rowid_boundary))
    }

    fn promote<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        split_at: CellIndex,
        _pager: &mut Pager<V>,
    ) -> SqliteResult<Promotion> {
        let i = split_at - 1;
        let cell = page.cell(i)?;
        Ok(Promotion {
            divider: Divider::RowId(cell.rowid_boundary),
            consumed: Consumed::LastOfLeft,
            left_rmp: Some(cell.left_child),
        })
    }
}

impl InteriorPolicy for TableInterior {
    fn child_of<B: AsRef<[u8]>>(page: &TypedPage<B, Self>, i: CellIndex) -> SqliteResult<PageNo> {
        Ok(page.cell(i)?.left_child)
    }

    fn right_divider(_old_cell: &[u8], _old_key: Value<'static>, right_bound: Divider) -> Divider {
        right_bound
    }
}

impl CellPolicy for TableLeaf {
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> SqliteResult<CellIndex> {
        let row_id = key.cast_int()? as u64;
        let (_, idx) = search_row_ids(page, pager, row_id)?;
        Ok(idx)
    }

    fn key_of<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        _pager: &mut Pager<V>,
    ) -> SqliteResult<Value<'static>> {
        Ok(Value::Integer(page.cell(i)?.row_id as i64))
    }

    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        _pager: &mut Pager<V>,
    ) -> SqliteResult<Divider> {
        Ok(Divider::RowId(page.cell(i)?.row_id))
    }

    fn promote<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        split_at: CellIndex,
        _pager: &mut Pager<V>,
    ) -> SqliteResult<Promotion> {
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

impl CellPolicy for IndexInterior {
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> SqliteResult<CellIndex> {
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
    ) -> SqliteResult<Value<'static>> {
        page.index_payload_key(i, pager)
    }

    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> SqliteResult<Divider> {
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
    ) -> SqliteResult<Promotion> {
        let i = split_at - 1;
        let left_child = page.cell(i)?.left_child;
        Ok(Promotion {
            divider: Self::divider_of_cell(page, i, pager)?,
            consumed: Consumed::LastOfLeft,
            left_rmp: Some(left_child),
        })
    }
}

impl InteriorPolicy for IndexInterior {
    fn child_of<B: AsRef<[u8]>>(page: &TypedPage<B, Self>, i: CellIndex) -> SqliteResult<PageNo> {
        Ok(page.cell(i)?.left_child)
    }

    fn right_divider(old_cell: &[u8], old_key: Value<'static>, _right_bound: Divider) -> Divider {
        Divider::Entry {
            cell: old_cell[4..].to_vec().into(),
            key: old_key,
        }
    }
}

impl CellPolicy for IndexLeaf {
    fn slot_for<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        pager: &mut Pager<V>,
        key: &Value,
    ) -> SqliteResult<CellIndex> {
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
    ) -> SqliteResult<Value<'static>> {
        page.index_payload_key(i, pager)
    }

    fn divider_of_cell<B: AsRef<[u8]>, V: Vfs>(
        page: &TypedPage<B, Self>,
        i: CellIndex,
        pager: &mut Pager<V>,
    ) -> SqliteResult<Divider> {
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
    ) -> SqliteResult<Promotion> {
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

impl RebalancePolicy for TableLeaf {
    fn pull_down(
        _sep_cell: &[u8],
        _left_rmp: Option<PageNo>,
        _usable: usize,
    ) -> SqliteResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn redistribute(
        left_share: &[Vec<u8>],
        _right_share: &[Vec<u8>],
        left_page: PageNo,
        _left_rmp: Option<PageNo>,
        _sep_cell: &[u8],
        usable: usize,
    ) -> SqliteResult<Redistribute> {
        let last = left_share
            .last()
            .ok_or_else(|| SqliteError::Internal("redistribute: empty left share".into()))?;
        let row_id = TableLeafCell::parse(last, usable)?.row_id;
        Ok(Redistribute {
            parent_cell: Encode::encode_table_interior_cell(left_page, row_id),
            drop_left_last: false,
            left_rmp: None,
        })
    }
}

impl RebalancePolicy for IndexLeaf {
    fn pull_down(
        sep_cell: &[u8],
        _left_rmp: Option<PageNo>,
        _usable: usize,
    ) -> SqliteResult<Option<Vec<u8>>> {
        Ok(Some(sep_cell[4..].to_vec()))
    }

    fn redistribute(
        left_share: &[Vec<u8>],
        _right_share: &[Vec<u8>],
        left_page: PageNo,
        _left_rmp: Option<PageNo>,
        _sep_cell: &[u8],
        _usable: usize,
    ) -> SqliteResult<Redistribute> {
        let promoted = left_share
            .last()
            .ok_or_else(|| SqliteError::Internal("redistribute: empty left share".into()))?;
        Ok(Redistribute {
            parent_cell: Encode::encode_index_interior_cell(left_page, promoted),
            drop_left_last: true,
            left_rmp: None,
        })
    }
}

impl RebalancePolicy for TableInterior {
    fn pull_down(
        sep_cell: &[u8],
        left_rmp: Option<PageNo>,
        usable: usize,
    ) -> SqliteResult<Option<Vec<u8>>> {
        let left_rmp = left_rmp.ok_or_else(|| {
            SqliteError::Internal("pull_down: left interior has no right child".into())
        })?;
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
    ) -> SqliteResult<Redistribute> {
        let promoted = left_share
            .last()
            .ok_or_else(|| SqliteError::Internal("redistribute: empty left share".into()))?;
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

impl RebalancePolicy for IndexInterior {
    fn pull_down(
        sep_cell: &[u8],
        left_rmp: Option<PageNo>,
        _usable: usize,
    ) -> SqliteResult<Option<Vec<u8>>> {
        let left_rmp = left_rmp.ok_or_else(|| {
            SqliteError::Internal("pull_down: left interior has no right child".into())
        })?;
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
    ) -> SqliteResult<Redistribute> {
        let promoted = left_share
            .last()
            .ok_or_else(|| SqliteError::Internal("redistribute: empty left share".into()))?;
        let promoted_cell = IndexInteriorCell::parse(promoted, usable)?;
        Ok(Redistribute {
            parent_cell: Encode::encode_index_interior_cell(left_page, &promoted[4..]),
            drop_left_last: true,
            left_rmp: Some(promoted_cell.left_child),
        })
    }
}

use crate::SqliteResult;
use crate::errors::{CorruptError, SqliteError};
use crate::pager::guard::PageGuard;
use crate::pager::pager::{PageNo, Pager};
use crate::storage::page::{BTreePage, BTreePageType, PageRef};
use crate::vfs::Vfs;

use super::kind::{IndexInterior, IndexLeaf, PageKind, TableInterior, TableLeaf, TypedPage};

pub enum AnyPageMut<B> {
    TableInterior(TypedPage<B, TableInterior>),
    TableLeaf(TypedPage<B, TableLeaf>),
    IndexInterior(TypedPage<B, IndexInterior>),
    IndexLeaf(TypedPage<B, IndexLeaf>),
}

impl<B: AsRef<[u8]> + AsMut<[u8]>> AnyPageMut<B> {
    pub(crate) fn parse(
        page_no: PageNo,
        page_size: usize,
        usable_size: usize,
        bytes: B,
    ) -> SqliteResult<Self> {
        let page = BTreePage::new(page_no, page_size, usable_size, bytes)?;
        Ok(match page.page_type()?.as_byte() {
            TableInterior::BYTE => Self::TableInterior(TypedPage::wrap(page)),
            TableLeaf::BYTE => Self::TableLeaf(TypedPage::wrap(page)),
            IndexInterior::BYTE => Self::IndexInterior(TypedPage::wrap(page)),
            IndexLeaf::BYTE => Self::IndexLeaf(TypedPage::wrap(page)),
            other => return Err(SqliteError::InvalidPageType(other)),
        })
    }

    pub(crate) fn no_of_cells(&self) -> SqliteResult<u16> {
        match self {
            Self::TableInterior(p) => p.no_of_cells(),
            Self::TableLeaf(p) => p.no_of_cells(),
            Self::IndexInterior(p) => p.no_of_cells(),
            Self::IndexLeaf(p) => p.no_of_cells(),
        }
    }
}

impl<B: AsRef<[u8]> + AsMut<[u8]>, K: PageKind> TypedPage<B, K> {
    pub(crate) fn parse_mut(
        page_no: PageNo,
        page_size: usize,
        usable_size: usize,
        bytes: B,
    ) -> SqliteResult<Self> {
        let page = BTreePage::new(page_no, page_size, usable_size, bytes)?;
        if page.page_type()?.as_byte() != K::BYTE {
            return Err(SqliteError::Corrupt(CorruptError::UnexpectedPageKind {
                page: page_no,
                expected: kind_name::<K>(),
            }));
        }
        Ok(TypedPage::wrap(page))
    }

    pub(crate) fn fresh(
        page_no: PageNo,
        page_size: usize,
        usable_size: usize,
        bytes: B,
    ) -> SqliteResult<Self> {
        let page_type = BTreePageType::try_from(K::BYTE)?;
        let page =
            BTreePage::new_from_raw_bytes(page_no, page_type, bytes, page_size, usable_size)?;
        Ok(TypedPage::wrap(page))
    }
}

pub(crate) fn parse_ref<'b, K: PageKind, V: Vfs>(
    page_no: PageNo,
    guard: &'b PageGuard,
    pager: &Pager<V>,
) -> SqliteResult<TypedPage<&'b [u8], K>> {
    let page = PageRef::new(
        page_no,
        pager.page_size(),
        pager.usable_size(),
        guard.bytes(),
    )?;
    if page.page_type()?.as_byte() != K::BYTE {
        return Err(SqliteError::Corrupt(CorruptError::UnexpectedPageKind {
                page: page_no,
                expected: kind_name::<K>(),
            }));
    }
    Ok(TypedPage::wrap(page))
}

fn kind_name<K: PageKind>() -> &'static str {
    match K::BYTE {
        TableInterior::BYTE => "table interior",
        TableLeaf::BYTE => "table leaf",
        IndexInterior::BYTE => "index interior",
        IndexLeaf::BYTE => "index leaf",
        _ => "unknown",
    }
}

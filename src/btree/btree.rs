use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::pager::guard::PageGuard;
struct BTreePageRef<'p> {
    page: NonNull<[u8]>,
    _marker: PhantomData<&'p PageGuard>,
}

impl<'p> BTreePageRef<'p> {
    fn new(page: NonNull<[u8]>, _guard: &'p PageGuard) -> Self {
        Self {
            page,
            _marker: PhantomData,
        }
    }
}

use std::ops::{Deref, DerefMut};

use super::raw_journal::{JournalMeta, RawJournal};

pub struct Journal {
    inner: Option<RawJournal>,
}
impl Journal {
    pub fn new(journal_metadata: JournalMeta) -> Self {
        let inner = RawJournal::new(journal_metadata);
        Self { inner: Some(inner) }
    }
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }
    pub fn uninit() -> Self {
        Self { inner: None }
    }
}

impl Deref for Journal {
    type Target = RawJournal;
    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().unwrap()
    }
}
impl DerefMut for Journal {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().unwrap()
    }
}

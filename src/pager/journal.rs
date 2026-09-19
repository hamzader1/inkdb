use std::ops::{Deref, DerefMut};

use crate::errors::SqliteError;
use crate::vfs::disk::DiskFile;

use super::raw_journal::{JournalMeta, RawJournal};

#[derive(Debug, Default)]
pub enum Journal {
    #[default]
    Disabled,
    Idle(RawJournal),
    Open {
        raw: RawJournal,
        file: DiskFile, /*Replace with VFs*/
        durable: u32,
    },
}

impl Journal {
    pub fn open(self, journal_metadata: JournalMeta) -> Self {
        if let Self::Disabled = self {
            let raw = RawJournal::new(journal_metadata);
            return Self::Idle(raw);
        }
        self // either idle or already open
    }
    pub fn init(&mut self) -> Result<(), SqliteError> {
        if let Self::Idle(raw) = self {
            let mut raw = std::mem::take(raw);
            let file = raw.init()?;

            *self = Self::Open {
                raw,
                file,
                durable: 0,
            };
        }
        Ok(())
    }

    pub fn destroy_internal(&mut self) -> Result<(), SqliteError> {
        if let Self::Open { raw, file, durable } = self {
            raw.destroy_internal()?;
            raw.reset();
            *self = Self::Idle(std::mem::take(raw))
        }
        Ok(())
    }
    pub fn reset(&mut self) {
        match *self {
            Self::Idle(ref mut raw) => raw.reset(),
            Self::Open {
                ref mut raw,
                ref mut durable,
                ..
            } => {
                raw.reset();
                *durable = 0
            }
            _ => {}
        }
    }
}

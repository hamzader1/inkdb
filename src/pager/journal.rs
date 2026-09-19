use crate::errors::SqliteError;
use crate::vfs::file::SqliteFile;

use super::raw_journal::{JournalMeta, RawJournal};

#[derive(Debug, Default)]
pub enum Journal<J: SqliteFile> {
    #[default]
    Disabled,
    Idle(RawJournal),
    Open {
        raw: RawJournal,
        file: J,
        durable: u32,
    },
}

impl<J: SqliteFile> Journal<J> {
    pub fn enable(&mut self, journal_metadata: JournalMeta) {
        if let Self::Disabled = self {
            *self = Self::Idle(RawJournal::new(journal_metadata));
        }
    }
    pub fn init(&mut self, file: J) -> Result<(), SqliteError> {
        if let Self::Idle(_) = self {
            let Self::Idle(raw) = std::mem::replace(self, Self::Disabled) else {
                unreachable!()
            };
            let mut raw = raw;
            raw.init(&file)?;
            *self = Self::Open {
                raw,
                file,
                durable: 0,
            };
        }
        Ok(())
    }

    pub fn persist_tail(&mut self) -> Result<(), SqliteError> {
        if let Self::Open { raw, file, durable } = self {
            let start = super::raw_journal::JOURNAL_HEADER_SIZE
                + *durable as usize * (raw.page_size as usize + 4);
            raw.persist_tail(file, start)?;
            *durable = raw.page_count;
        }

        Ok(())
    }

    pub fn into_idle(&mut self) {
        if let Self::Open { .. } = self {
            let Self::Open { mut raw, .. } = std::mem::replace(self, Self::Disabled) else {
                unreachable!()
            };
            raw.reset();
            *self = Self::Idle(raw);
        }
    }
    pub fn reset(&mut self) {
        match self {
            Self::Idle(raw) => raw.reset(),
            Self::Open { raw, durable, .. } => {
                raw.reset();
                *durable = 0
            }
            Self::Disabled => {}
        }
    }
}

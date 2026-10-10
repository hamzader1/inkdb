use crate::errors::InkError;
use crate::vfs::file::InkFile;

use super::raw_journal::{JournalMeta, RawJournal};

/// The journal goes through these states in order. It starts disabled, moves to
/// idle once a write transaction begins and a journal is worth keeping, and
/// opens when the first changed page has to be written down. When the last
/// change is made it either commits, which makes the changes in the database
/// itself, or rolls back, which puts the pages back the way the journal
/// remembers them.
#[derive(Debug, Default)]
pub(crate) enum Journal<J: InkFile> {
    /// No write is under way, so there is nothing to journal.
    #[default]
    Disabled,
    /// A write is under way but no page has been changed yet. The journal is
    /// ready, with the old database size noted, but its file is not open.
    Idle(RawJournal),
    /// Pages are being changed. The journal file is open and `durable` says how
    /// many of its records have been written to disk.
    Open {
        /// The journal's bytes, with a record for each page changed so far.
        raw: RawJournal,
        /// The open journal file.
        file: J,
        /// How many of the journal's records have reached disk.
        durable: u32,
    },
}

impl<J: InkFile> Journal<J> {
    /// Make the journal ready for a write. Nothing happens if it is already
    /// enabled, which keeps a second `enable` from throwing away records a
    /// write in progress has already made.
    pub fn enable(&mut self, journal_metadata: JournalMeta) {
        if let Self::Disabled = self {
            *self = Self::Idle(RawJournal::new(journal_metadata));
        }
    }
    /// Note the size the database had before the write began, which is the
    /// size a rollback has to bring the file back to.
    pub fn record_db_size(&mut self, db_size: u32) {
        if let Self::Idle(raw) = self {
            raw.set_db_size(db_size);
        }
    }
    /// Take the open journal file and move to the open state, writing the
    /// journal header out so a crash from here on leaves something recovery
    /// can use.
    pub fn init(&mut self, file: J) -> Result<(), InkError> {
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

    /// Write the journal records that are not yet on disk, all of them from the
    /// last one that was.
    pub fn persist_tail(&mut self) -> Result<(), InkError> {
        if let Self::Open { raw, file, durable } = self {
            let start = super::raw_journal::JOURNAL_HEADER_SIZE
                + *durable as usize * (raw.page_size as usize + 4);
            raw.persist_tail(file, start)?;
            *durable = raw.page_count;
        }

        Ok(())
    }

    /// Close the journal and go back to idle, forgetting the records it holds.
    /// This is what a commit does with it once the changes are safe in the
    /// database.
    pub fn set_to_idle(&mut self) {
        if let Self::Open { .. } = self {
            let Self::Open { mut raw, .. } = std::mem::replace(self, Self::Disabled) else {
                unreachable!()
            };
            raw.reset();
            *self = Self::Idle(raw);
        }
    }
    /// Throw away everything the journal has recorded.
    #[allow(dead_code)]
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

use crate::SqliteResult;
use crate::errors::CorruptError;
use crate::record::{Record, Value};

pub mod aggregate;
pub mod context;
pub mod create;
pub mod delete;
pub mod eval;
pub mod filter;
pub mod index;
pub mod insert;
pub mod limit;
pub mod prepare;
pub mod project;
pub mod scan_guard;
pub mod tablescan;
pub mod transaction;
pub mod truncate;

#[derive(Debug)]
pub enum Columns {
    Stored(Vec<u8>),
    Computed(Vec<Value<'static>>),
}

#[derive(Debug)]
pub struct Row {
    key: u64,
    rowid_column: Option<usize>,
    columns: Columns,
}

impl Row {
    pub fn new(key: u64, data: Vec<Value<'static>>) -> Self {
        Self {
            key,
            rowid_column: None,
            columns: Columns::Computed(data),
        }
    }

    pub fn stored(key: u64, record: Vec<u8>) -> Self {
        Self {
            key,
            rowid_column: None,
            columns: Columns::Stored(record),
        }
    }

    pub fn stored_with_rowid(key: u64, record: Vec<u8>, rowid_column: Option<usize>) -> Self {
        Self {
            key,
            rowid_column,
            columns: Columns::Stored(record),
        }
    }

    pub fn rowid_column(&self) -> Option<usize> {
        self.rowid_column
    }

    pub fn key(&self) -> u64 {
        self.key
    }

    pub fn record(&self) -> SqliteResult<Option<Record<'_>>> {
        match (&self.columns, self.rowid_column) {
            (Columns::Stored(bytes), None) => Ok(Some(Record::new(bytes)?)),
            _ => Ok(None),
        }
    }

    pub fn value(&self, index: usize) -> SqliteResult<Value<'_>> {
        if self.rowid_column == Some(index) {
            return Ok(Value::Integer(self.key as i64));
        }
        match &self.columns {
            Columns::Stored(bytes) => Record::new(bytes)?.value(index),
            Columns::Computed(values) => values.get(index).cloned().ok_or_else(|| {
                CorruptError::NoSuchField {
                    field: index,
                    fields: values.len(),
                }
                .into()
            }),
        }
    }

    pub fn len(&self) -> usize {
        match &self.columns {
            Columns::Stored(bytes) => match Record::new(bytes) {
                Ok(record) => record.len(),
                Err(_) => 0,
            },
            Columns::Computed(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn to_values(&self) -> SqliteResult<Vec<Value<'static>>> {
        match (&self.columns, self.rowid_column) {
            (Columns::Computed(values), _) => Ok(values.clone()),
            (Columns::Stored(_), None) => match self.record()? {
                Some(record) => record.to_values_owned(),
                None => Ok(Vec::new()),
            },
            (Columns::Stored(_), Some(_)) => (0..self.len())
                .map(|index| Ok(self.value(index)?.into_static()))
                .collect(),
        }
    }
}

pub struct RowView<'a> {
    key: u64,
    rowid_column: Option<usize>,
    record: Record<'a>,
}

impl<'a> RowView<'a> {
    pub fn new(key: u64, record: Record<'a>, rowid_column: Option<usize>) -> Self {
        Self {
            key,
            rowid_column,
            record,
        }
    }
}

pub trait ColumnSource {
    fn column(&self, index: usize) -> SqliteResult<Value<'_>>;
    fn column_count(&self) -> usize;
}

impl ColumnSource for RowView<'_> {
    fn column(&self, index: usize) -> SqliteResult<Value<'_>> {
        if self.rowid_column == Some(index) {
            return Ok(Value::Integer(self.key as i64));
        }
        self.record.value(index)
    }

    fn column_count(&self) -> usize {
        self.record.len()
    }
}

impl ColumnSource for Row {
    fn column(&self, index: usize) -> SqliteResult<Value<'_>> {
        self.value(index)
    }

    fn column_count(&self) -> usize {
        self.len()
    }
}

impl ColumnSource for Record<'_> {
    fn column(&self, index: usize) -> SqliteResult<Value<'_>> {
        self.value(index)
    }

    fn column_count(&self) -> usize {
        self.len()
    }
}

impl ColumnSource for [Value<'static>] {
    fn column(&self, index: usize) -> SqliteResult<Value<'_>> {
        self.get(index).cloned().ok_or_else(|| {
            CorruptError::NoSuchField {
                field: index,
                fields: self.len(),
            }
            .into()
        })
    }

    fn column_count(&self) -> usize {
        self.len()
    }
}

pub struct RowWrapper(pub Row);

impl std::fmt::Display for RowWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.record().map_err(|_| std::fmt::Error)? {
            Some(record) => {
                for (i, value) in record.values().enumerate() {
                    write!(f, "{}", value.map_err(|_| std::fmt::Error)?)?;
                    if i + 1 < record.len() {
                        write!(f, ", ")?;
                    }
                }
            }
            None => {
                let values = self.0.to_values().map_err(|_| std::fmt::Error)?;
                for (i, value) in values.iter().enumerate() {
                    write!(f, "{value}")?;
                    if i + 1 < values.len() {
                        write!(f, ", ")?;
                    }
                }
            }
        }
        Ok(())
    }
}

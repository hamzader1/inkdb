use std::fs::File;

use crate::errors::CorruptError;
use crate::record::{Record, Value};
use crate::{InkResult, MemCursor};

use self::sort::load_page;

pub mod aggregate;
pub mod context;
pub mod create;
pub mod delete;
pub mod eval;
pub mod filter;
pub mod index;
pub mod insert;
pub mod limit;
pub mod materialized;
pub mod prepare;
pub mod project;
pub mod scan_guard;
pub mod sort;
pub mod tablescan;
pub mod transaction;
pub mod truncate;
pub mod update;

pub(crate) const MEM_CAP: usize = 0xA00000; /*10MiB*/

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

    pub fn stored_bytes(&self) -> Option<&[u8]> {
        match &self.columns {
            Columns::Stored(bytes) => Some(bytes),
            Columns::Computed(_) => None,
        }
    }

    pub fn raw_record(&self) -> InkResult<Option<Record<'_>>> {
        match self.stored_bytes() {
            Some(bytes) => Ok(Some(Record::new(bytes)?)),
            None => Ok(None),
        }
    }

    pub fn value(&self, index: usize) -> InkResult<Value<'_>> {
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

    pub fn to_values(&self) -> InkResult<Vec<Value<'static>>> {
        match (&self.columns, self.rowid_column) {
            (Columns::Computed(values), _) => Ok(values.clone()),
            (Columns::Stored(_), None) => match self.raw_record()? {
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
    fn column(&self, index: usize) -> InkResult<Value<'_>>;
    fn column_count(&self) -> usize;
}

impl ColumnSource for RowView<'_> {
    fn column(&self, index: usize) -> InkResult<Value<'_>> {
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
    fn column(&self, index: usize) -> InkResult<Value<'_>> {
        self.value(index)
    }

    fn column_count(&self) -> usize {
        self.len()
    }
}

impl ColumnSource for Record<'_> {
    fn column(&self, index: usize) -> InkResult<Value<'_>> {
        self.value(index)
    }

    fn column_count(&self) -> usize {
        self.len()
    }
}

impl ColumnSource for [Value<'static>] {
    fn column(&self, index: usize) -> InkResult<Value<'_>> {
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
        let columns = self.0.len();
        for index in 0..columns {
            let value = self.0.value(index).map_err(|_| std::fmt::Error)?;
            write!(f, "{value}")?;
            if index + 1 < columns {
                write!(f, ", ")?;
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum StreamSource {
    Mem {
        buffer: Vec<u8>,
        offset: usize,
        nrows: usize,
    },
    Disk {
        f: File,
        buffer: Vec<u8>,
        file_offset: usize,
        buffer_offset: usize,
        page_size: usize,
        nrows: u32,
        rrows: usize,
    },
    None,
}

impl StreamSource {
    fn yield_from_stream(&mut self, nread: &mut usize) -> InkResult<Option<Row>> {
        match self {
            &mut StreamSource::Mem {
                ref mut buffer,
                ref mut offset,
                nrows,
            } => {
                if *nread == nrows {
                    return Ok(None);
                }
                let buffer = &buffer[*offset..];
                let mut cursor = MemCursor::new(buffer);
                let (len, consumed) = cursor.read_next_varint(buffer.len())?;
                let row = Row::stored(0, buffer[consumed..consumed + len as usize].into());
                *nread += 1;
                *offset += consumed + len as usize;

                Ok(Some(row))
            }
            &mut StreamSource::Disk {
                ref mut f,
                ref mut buffer,
                ref mut buffer_offset,
                ref mut file_offset,
                ref mut rrows,
                nrows,
                page_size,
            } => {
                if *nread == nrows as usize {
                    return Ok(None);
                }
                if *buffer_offset == buffer.len() {
                    let remaining = nrows as usize - *rrows;
                    if remaining == 0 {
                        return Ok(None);
                    }
                    let r = load_page(f, buffer, remaining, page_size, file_offset)?;
                    if r == 0 {
                        return Ok(None);
                    }
                    *rrows += r;
                    *buffer_offset = 0;
                }
                let buffer = &buffer[*buffer_offset..];
                let mut cursor = MemCursor::new(buffer);
                let (len, consumed) = cursor.read_next_varint(buffer.len())?;
                let row = Row::stored(0, buffer[consumed..consumed + len as usize].to_vec());
                *buffer_offset += consumed + len as usize;
                *nread += 1;
                Ok(Some(row))
            }
            StreamSource::None => Ok(None),
        }
    }
}

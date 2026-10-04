use crate::errors::CorruptError;
use crate::record::{Record, Value};
use crate::varint::{decode_varint, encode_varint};
use crate::vfs::Vfs;
use crate::{InkResult, MemCursor};

use self::sort::load_page;

pub mod aggregate;
pub mod context;
pub mod create;
pub mod delete;
pub mod drop;
pub mod eval;
pub mod filter;
pub mod index;
pub mod insert;
pub mod limit;
pub mod materialized;
pub mod prepare;
pub mod project;
pub mod rowid;
pub mod scan_guard;
pub mod sort;
pub mod tablescan;
pub mod transaction;
pub mod truncate;
pub mod update;

pub(crate) const MEM_CAP: usize = 0xA00000; /*10MiB*/

#[derive(Debug)]
pub(crate) enum Columns {
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
impl<'a> ColumnSource for Vec<Value<'a>> {
    fn column(&self, index: usize) -> InkResult<Value<'_>> {
        let v = self.get(index).ok_or(CorruptError::NoSuchField {
            field: index,
            fields: self.len(),
        })?;
        Ok(v.to_owned_static())
    }
    fn column_count(&self) -> usize {
        self.len()
    }
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

pub(crate) fn frame_len(record_len: usize, key: u64) -> usize {
    let mut buffer = [0u8; 9];
    let key_bytes = encode_varint(&mut buffer, key);
    let payload = key_bytes + record_len;
    let length_bytes = encode_varint(&mut buffer, payload as u64);
    length_bytes + payload
}

pub(crate) fn encode_frame(out: &mut Vec<u8>, key: u64, record: &[u8]) -> usize {
    let mut length_buffer = [0u8; 9];
    let mut key_buffer = [0u8; 9];
    let key_bytes = encode_varint(&mut key_buffer, key);
    let payload = key_bytes + record.len();
    let length_bytes = encode_varint(&mut length_buffer, payload as u64);
    out.extend_from_slice(&length_buffer[..length_bytes]);
    out.extend_from_slice(&key_buffer[..key_bytes]);
    out.extend_from_slice(record);
    length_bytes + payload
}

pub(crate) fn decode_frame(frame: &[u8]) -> InkResult<(u64, &[u8])> {
    let (payload, consumed) = decode_varint(frame).ok_or(CorruptError::TruncatedRecord {
        field: 0,
        size: frame.len(),
        available: frame.len(),
    })?;
    let rest = &frame[consumed..];
    let (key, key_bytes) = decode_varint(rest).ok_or(CorruptError::TruncatedRecord {
        field: 0,
        size: frame.len(),
        available: frame.len(),
    })?;
    let start = key_bytes;
    let end = payload as usize;
    if end > rest.len() || start > end {
        return Err(CorruptError::TruncatedRecord {
            field: 0,
            size: consumed + end,
            available: frame.len(),
        }
        .into());
    }
    Ok((key, &rest[start..end]))
}

#[derive(Debug)]
pub(crate) enum StreamSource<V: Vfs> {
    Mem {
        buffer: Vec<u8>,
        offset: usize,
        nrows: usize,
    },
    Disk {
        f: V::File,
        buffer: Vec<u8>,
        file_offset: usize,
        buffer_offset: usize,
        page_size: usize,
        nrows: u32,
        rrows: usize,
    },
    None,
}

impl<V: Vfs> StreamSource<V> {
    fn yield_from_stream(
        &mut self,
        nread: &mut usize,
        rowid_column: Option<usize>,
    ) -> InkResult<Option<Row>> {
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
                let frame_len = consumed + len as usize;
                let (key, record) = decode_frame(&buffer[..frame_len])?;
                let row = Row::stored_with_rowid(key, record.to_vec(), rowid_column);
                *nread += 1;
                *offset += frame_len;

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
                let frame_len = consumed + len as usize;
                let (key, record) = decode_frame(&buffer[..frame_len])?;
                let row = Row::stored_with_rowid(key, record.to_vec(), rowid_column);
                *buffer_offset += frame_len;
                *nread += 1;
                Ok(Some(row))
            }
            StreamSource::None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tuple::Tuple;

    fn record_of(values: &[Value]) -> Vec<u8> {
        Tuple::serialize(values)
    }

    #[test]
    fn frames_carry_the_whole_key() {
        let record = record_of(&[Value::Integer(7), Value::Text("x".into())]);
        let key = 4_294_967_297u64;
        let mut buffer = Vec::new();
        let written = encode_frame(&mut buffer, key, &record);
        assert_eq!(written, frame_len(record.len(), key));
        assert_eq!(written, buffer.len());
        let (decoded_key, decoded) = decode_frame(&buffer).expect("frame");
        assert_eq!(decoded_key, key);
        assert_eq!(decoded, record.as_slice());
    }

    #[test]
    fn frames_can_be_walked_back_to_back() {
        let keys = [0u64, 1, 300, 4_294_967_297];
        let mut buffer = Vec::new();
        for key in keys {
            let record = record_of(&[Value::Integer(key as i64)]);
            encode_frame(&mut buffer, key, &record);
        }
        let mut offset = 0;
        let mut seen = Vec::new();
        while offset < buffer.len() {
            let (length, consumed) = decode_varint(&buffer[offset..]).expect("length");
            let end = offset + consumed + length as usize;
            let (key, record) = decode_frame(&buffer[offset..end]).expect("frame");
            seen.push(key);
            assert_eq!(
                Record::new(record)
                    .expect("record")
                    .value(0)
                    .expect("value"),
                Value::Integer(key as i64)
            );
            offset = end;
        }
        assert_eq!(seen, keys);
    }

    #[test]
    fn truncated_frames_are_an_error_not_a_panic() {
        let record = record_of(&[Value::Integer(1)]);
        let mut buffer = Vec::new();
        encode_frame(&mut buffer, 9, &record);
        assert!(decode_frame(&buffer[..buffer.len() - 1]).is_err());
        assert!(decode_frame(&[]).is_err());
    }

    #[test]
    fn a_stream_restores_the_key_and_the_rowid_override() {
        let rows = [
            Row::stored_with_rowid(
                11,
                record_of(&[Value::Null, Value::Text("c".into())]),
                Some(0),
            ),
            Row::stored_with_rowid(
                22,
                record_of(&[Value::Null, Value::Text("a".into())]),
                Some(0),
            ),
        ];
        let mut buffer = Vec::new();
        for row in &rows {
            encode_frame(&mut buffer, row.key(), row.stored_bytes().expect("bytes"));
        }
        let mut source = StreamSource::<crate::vfs::disk::DiskVfs>::Mem {
            buffer,
            offset: 0,
            nrows: rows.len(),
        };
        let mut nread = 0;
        let mut seen = Vec::new();
        while let Some(row) = source
            .yield_from_stream(&mut nread, Some(0))
            .expect("yield")
        {
            seen.push((row.key(), row.value(0).expect("value").into_static()));
        }
        assert_eq!(
            seen,
            vec![(11, Value::Integer(11)), (22, Value::Integer(22))]
        );
    }
}

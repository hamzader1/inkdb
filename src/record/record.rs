use std::borrow::Cow;

use crate::InkResult;
use crate::errors::CorruptError;
use crate::varint::decode_varint;

use super::tuple::{Tuple, decode_sqltype, into_borrowed};
use super::{SERIAL_BLOB_MIN, SERIAL_TEXT_MIN, Value};

/// A record as it is stored in a B-tree: a row of values backed by raw bytes.
///
/// A record doesn’t own the bytes it reads from. Instead, it keeps just enough
/// information to locate each field, such as where the header ends, where the
/// serial types start, and how many fields the record contains. The fields are
/// decoded only when they are actually needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    bytes: &'a [u8],
    header_len: usize,
    first_serial_type: usize,
    fields: usize,
}

impl<'a> Record<'a> {
    pub fn new(bytes: &'a [u8]) -> InkResult<Self> {
        let (header_len, consumed) = decode_varint(bytes).ok_or(CorruptError::RecordHeader {
            claimed: 0,
            len: bytes.len(),
        })?;
        let header_len = header_len as usize;
        if header_len > bytes.len() || header_len < consumed {
            return Err(CorruptError::RecordHeader {
                claimed: header_len,
                len: bytes.len(),
            }
            .into());
        }
        let mut field = 0;
        let mut pos = consumed;
        let mut payload_len = 0;
        while pos < header_len {
            let (serial_type, used) =
                decode_varint(&bytes[pos..]).ok_or(CorruptError::RecordHeader {
                    claimed: header_len,
                    len: bytes.len(),
                })?;
            if pos + used > header_len {
                return Err(CorruptError::RecordHeader {
                    claimed: header_len,
                    len: bytes.len(),
                }
                .into());
            }
            if serial_type == 10 || serial_type == 11 {
                return Err(CorruptError::ReservedSerialType {
                    serial_type: serial_type as u8,
                    field,
                }
                .into());
            }
            payload_len += Tuple::content_meta(serial_type).size;
            pos += used;
            field += 1;
        }
        let available = bytes.len() - header_len;
        if payload_len > available {
            return Err(CorruptError::TruncatedRecord {
                field,
                size: payload_len,
                available,
            }
            .into());
        }
        Ok(Self {
            bytes,
            header_len,
            first_serial_type: consumed,
            fields: field,
        })
    }

    /// How many fields the record holds.
    pub fn len(&self) -> usize {
        self.fields
    }

    /// Whether the record holds no fields at all.
    pub fn is_empty(&self) -> bool {
        self.fields == 0
    }

    /// How many bytes the header takes, with the payload starting right after
    /// it.
    pub fn header_len(&self) -> usize {
        self.header_len
    }

    /// The bytes the record.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The serial type of one field, which says what it holds and how long it
    /// is.
    pub fn serial_type(&self, field: usize) -> InkResult<u64> {
        Ok(self.field_span(field)?.0)
    }

    /// The stored bytes of one field, left undecoded.
    pub fn field_bytes(&self, field: usize) -> InkResult<&'a [u8]> {
        let (_, start, size) = self.field_span(field)?;
        Ok(&self.bytes[start..start + size])
    }

    /// Read a field and return it as a value.
    ///
    /// Text and blobs borrow their data from the record’s bytes, so they are only
    /// valid for as long as the record itself is alive.
    pub fn value(&self, field: usize) -> InkResult<Value<'a>> {
        let (serial_type, start, size) = self.field_span(field)?;
        let meta = Tuple::content_meta(serial_type);
        let payload = &self.bytes[start..start + size];
        match meta.serial_type {
            SERIAL_TEXT_MIN => match std::str::from_utf8(payload) {
                Ok(text) => Ok(Value::Text(Cow::Borrowed(text))),
                Err(_) => Err(CorruptError::InvalidUtf8 {
                    field,
                    fields: self.fields,
                }
                .into()),
            },
            SERIAL_BLOB_MIN => Ok(Value::Blob(Cow::Borrowed(payload))),
            _ => Ok(into_borrowed(decode_sqltype(payload, &meta))),
        }
    }

    /// Read one field as a value that owns its text and blob, so it can outlive
    /// the record.
    pub fn value_owned(&self, field: usize) -> InkResult<Value<'static>> {
        Ok(self.value(field)?.into_static())
    }

    /// Read the fields in order, one result per field.
    ///
    /// A field that cannot be decoded shows up as an error in its own place.
    pub fn values<'b>(&'b self) -> impl Iterator<Item = InkResult<Value<'a>>> + 'b {
        (0..self.fields).map(|field| self.value(field))
    }

    /// Read every field into a list.
    pub fn to_values(&self) -> InkResult<Vec<Value<'a>>> {
        self.values().collect()
    }

    /// Read every field into a list, copying the text and blobs on the way.
    pub fn to_values_owned(&self) -> InkResult<Vec<Value<'static>>> {
        Ok(self
            .to_values()?
            .into_iter()
            .map(Value::into_static)
            .collect())
    }

    /// The last field, or nothing at all when the record is empty.
    pub fn last(&self) -> Option<InkResult<Value<'a>>> {
        match self.fields {
            0 => None,
            fields => Some(self.value(fields - 1)),
        }
    }

    /// Get the serial type, payload offset, and payload length for a field.
    ///
    /// The header is walked from the beginning each time because a field’s
    /// position can only be found by adding up the lengths of the fields before it.
    /// The offset is relative to the start of the record, not the payload.
    fn field_span(&self, field: usize) -> InkResult<(u64, usize, usize)> {
        if field >= self.fields {
            return Err(CorruptError::NoSuchField {
                field,
                fields: self.fields,
            }
            .into());
        }
        let mut pos = self.first_serial_type;
        let mut start = self.header_len;
        let mut index = 0;
        while pos < self.header_len {
            let (serial_type, used) =
                decode_varint(&self.bytes[pos..]).ok_or(CorruptError::RecordHeader {
                    claimed: self.header_len,
                    len: self.bytes.len(),
                })?;
            let size = Tuple::content_meta(serial_type).size;
            if index == field {
                return Ok((serial_type, start, size));
            }
            pos += used;
            start += size;
            index += 1;
        }
        Err(CorruptError::NoSuchField {
            field,
            fields: self.fields,
        }
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Record;

    fn int(n: i64) -> Value<'static> {
        Value::Integer(n)
    }

    fn real(f: f64) -> Value<'static> {
        Value::Float(f)
    }

    fn text(s: &str) -> Value<'static> {
        Value::Text(Cow::Owned(s.to_string()))
    }

    fn blob(b: &[u8]) -> Value<'static> {
        Value::Blob(Cow::Owned(b.to_vec()))
    }

    fn rows() -> Vec<Vec<Value<'static>>> {
        vec![
            vec![],
            vec![Value::Null],
            vec![int(0)],
            vec![int(1)],
            vec![int(-1)],
            vec![int(127)],
            vec![int(128)],
            vec![int(-32768)],
            vec![int(70000)],
            vec![int(i64::MIN)],
            vec![int(i64::MAX)],
            vec![real(1.5), real(-0.25), real(0.0)],
            vec![text("")],
            vec![text("User-000001")],
            vec![blob(&[])],
            vec![blob(&[0xff, 0xfe, 0x00])],
            vec![text("Position-1"), int(21), real(30013.0), Value::Null],
            vec![text("a whole record"), int(-7), blob(&[1, 2, 3]), real(2.5)],
        ]
    }

    #[test]
    fn round_trips_every_encoding_the_writer_produces() {
        for values in rows() {
            let bytes = Tuple::serialize(&values);
            let record = Record::new(&bytes).expect("parse");
            assert_eq!(record.len(), values.len(), "{values:?}");
            let decoded = record.to_values().expect("decode");
            assert_eq!(decoded, values, "{values:?}");
        }
    }

    #[test]
    fn decodes_one_field_at_a_time() {
        let values = vec![text("Position-1"), int(21), real(30013.0), Value::Null];
        let bytes = Tuple::serialize(&values);
        let record = Record::new(&bytes).expect("parse");
        for (field, expected) in values.iter().enumerate() {
            assert_eq!(&record.value(field).expect("field"), expected);
        }
        assert_eq!(record.last().expect("last").expect("field"), Value::Null);
        assert_eq!(
            record.field_bytes(0).expect("bytes"),
            "Position-1".as_bytes()
        );
        assert_eq!(record.field_bytes(3).expect("bytes"), &[] as &[u8]);
        assert_eq!(record.serial_type(0).expect("serial"), 13 + 2 * 10);
        assert_eq!(record.serial_type(1).expect("serial"), 1);
        assert_eq!(record.serial_type(2).expect("serial"), 7);
        assert_eq!(record.serial_type(3).expect("serial"), 0);
        assert_eq!(record.value_owned(0).expect("owned"), values[0]);
        assert_eq!(record.to_values_owned().expect("owned"), values);
    }

    #[test]
    fn long_values_keep_their_serial_type() {
        for len in [121usize, 122, 125, 200, 255, 256, 1000] {
            let value = text(&"x".repeat(len));
            let bytes = Tuple::serialize(std::slice::from_ref(&value));
            let record = Record::new(&bytes).expect("parse");
            assert_eq!(
                record.serial_type(0).expect("serial"),
                13 + 2 * len as u64,
                "serial type of a {len} byte text"
            );
            assert_eq!(record.value(0).expect("value"), value, "{len} bytes");
        }
    }

    #[test]
    fn empty_records_have_no_fields() {
        let bytes = Tuple::serialize(&[]);
        let record = Record::new(&bytes).expect("parse");
        assert_eq!(record.len(), 0);
        assert!(record.is_empty());
        assert!(record.last().is_none());
        assert!(record.value(0).is_err());
        assert_eq!(record.to_values().expect("decode"), Vec::new());
    }

    #[test]
    fn corrupt_records_are_errors_not_panics() {
        assert!(Record::new(&[]).is_err());

        let bytes = Tuple::serialize(&[text("hello"), int(1)]);
        assert!(Record::new(&bytes[..bytes.len() - 1]).is_err());

        let header_says_more_than_there_is = [9u8, 1];
        assert!(Record::new(&header_says_more_than_there_is).is_err());

        let reserved_serial_type = [2u8, 10];
        assert!(Record::new(&reserved_serial_type).is_err());

        let truncated_header = [5u8, 1, 2, 3];
        assert!(Record::new(&truncated_header).is_err());
    }

    #[test]
    fn invalid_utf8_in_a_text_field_is_an_error() {
        let bytes = [2u8, 13 + 2 * 3, 0xff, 0xfe, 0xfd];
        let record = Record::new(&bytes).expect("parse");
        assert_eq!(record.len(), 1);
        assert!(record.value(0).is_err());
        assert_eq!(record.field_bytes(0).expect("bytes"), &[0xff, 0xfe, 0xfd]);
    }

    #[test]
    fn fields_past_the_end_are_an_error() {
        let bytes = Tuple::serialize(&[int(1)]);
        let record = Record::new(&bytes).expect("parse");
        assert!(record.value(1).is_err());
        assert!(record.serial_type(7).is_err());
        assert!(record.field_bytes(2).is_err());
    }
}

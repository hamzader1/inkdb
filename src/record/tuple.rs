use crate::varint::encode_varint;

use super::*;

pub struct Tuple;

impl Tuple {
    /// Describes a field’s type and how many bytes it uses.
    ///
    /// # Panics
    /// When the serial type is one a record header cannot carry. The reserved
    /// codes 10 and 11 are caught when the record is read, so they are only
    /// unreachable here.
    pub(crate) fn content_meta(serial_type: u64) -> RecordMetadata {
        let f = |st, sz| RecordMetadata::new(st, sz);
        match serial_type {
            0 => f(SERIAL_NULL, 0),
            1 => f(SERIAL_INT8, 1),
            2 => f(SERIAL_INT16, 2),
            3 => f(SERIAL_INT24, 3),
            4 => f(SERIAL_INT32, 4),
            5 => f(SERIAL_INT48, 6),
            6 => f(SERIAL_INT64, 8),
            7 => f(SERIAL_FLOAT64, 8),
            8 => f(SERIAL_INT0, 0),
            9 => f(SERIAL_INT1, 0),
            // 10 and 11 are reserved by SQLite and never reach this far.
            10 | 11 => unreachable!(),
            // BLOB
            n if n >= 12 && n % 2 == 0 => f(SERIAL_BLOB_MIN, ((n - 12) / 2) as usize),
            // TEXT
            n if n >= 13 && n % 2 == 1 => f(SERIAL_TEXT_MIN, ((n - 13) / 2) as usize),
            _ => unreachable!(),
        }
    }

    /// Write a value’s payload and return the serial type to use for it.
    ///
    /// The value’s bytes are appended to output. The serial type is returned
    /// separately because the header is built independently.
    ///
    /// # Panics
    /// When the value is a tuple, which has no serial type of its own and is
    /// never stored as a field.
    pub fn encode_sqltype(value: &Value, output: &mut Vec<u8>) -> usize {
        match value {
            Value::Integer(_) => {
                let compressed_int = CompressedNumeric::from(value);
                match compressed_int {
                    CompressedNumeric::I8(n) => {
                        output.extend_from_slice(&i8::to_be_bytes(n));
                        SERIAL_INT8 as _
                    }
                    CompressedNumeric::I16(n) => {
                        output.extend_from_slice(&i16::to_be_bytes(n));
                        SERIAL_INT16 as _
                    }
                    CompressedNumeric::I32(n) => {
                        output.extend_from_slice(&i32::to_be_bytes(n));
                        SERIAL_INT32 as _
                    }
                    CompressedNumeric::I64(n) => {
                        output.extend_from_slice(&i64::to_be_bytes(n));
                        SERIAL_INT64 as _
                    }
                    _ => unreachable!(),
                }
            }
            Value::Float(_) => {
                let compressed_float = CompressedNumeric::from(value);
                match compressed_float {
                    CompressedNumeric::F32(f) => {
                        output.extend_from_slice(&f64::to_be_bytes(f as _));
                        SERIAL_FLOAT64 as _
                    }
                    CompressedNumeric::F64(f) => {
                        output.extend_from_slice(&f64::to_be_bytes(f));
                        SERIAL_FLOAT64 as _
                    }
                    _ => unreachable!(),
                }
            }

            Value::Null => 0 as _,
            Value::Text(t) => {
                output.extend_from_slice(t.as_bytes());
                text_encoding(t.len())
            }
            Value::Blob(b) => {
                output.extend_from_slice(b);
                blob_encoding(b.len())
            }
            _ => unreachable!(),
        }
    }
}

/// The serial type for text of this many bytes. The length is doubled and added
/// to thirteen.
// SQLite uses the formula N = 13 + 2 * Z. We can reverse it to get N from Z:
// N = (2 * Z) + 13
const fn text_encoding(len: usize) -> usize {
    (len * 2) + 13
}

/// The serial type for a blob of this many bytes. The length is doubled and
/// added to twelve.
// Same for blob_encoding. See the formula above.
const fn blob_encoding(len: usize) -> usize {
    (len * 2) + 12
}

/// A field taken directly from the record’s bytes, before it is converted into
/// a [Value].
///
/// Text and blobs borrow from the payload they came from, so the decoded value
/// is tied to the lifetime of those bytes.
pub(crate) enum DecodedValue<'a> {
    /// NULL, which has no bytes behind it.
    Null,
    /// A whole number, already widened to `i64`.
    Integer(i64),
    /// A number with a fractional part.
    Float(f64),
    /// A run of bytes, borrowed from the payload.
    Blob(&'a [u8]),
    /// Text, borrowed from the payload.
    Text(&'a str),
}

/// Read a field’s bytes according to its serial type.
///
/// Integers shorter than eight bytes are sign extended to i64, as required by
/// the (our) format. This is why the bytes are placed at the high end of the buffer
/// and then read back as a whole.
///
/// # Panics
/// When the serial type cannot belong to a stored field, which the caller has
/// already ruled out, or when a text field does not hold valid UTF-8.
pub(crate) fn decode_sqltype<'a>(
    bytes: &'a [u8],
    record_metadata: &RecordMetadata,
) -> DecodedValue<'a> {
    let mut buf = [0u8; 8];

    match record_metadata.serial_type {
        0 => DecodedValue::Null,
        1 => {
            buf[7..8].copy_from_slice(bytes);
            DecodedValue::Integer(i64::from_be_bytes(buf))
        }
        2 => {
            buf[6..8].copy_from_slice(bytes);
            DecodedValue::Integer(i64::from_be_bytes(buf))
        }
        3 => {
            buf[5..8].copy_from_slice(bytes);
            DecodedValue::Integer(i64::from_be_bytes(buf))
        }
        4 => {
            buf[4..8].copy_from_slice(bytes);
            DecodedValue::Integer(i64::from_be_bytes(buf))
        }
        5 => {
            buf[2..8].copy_from_slice(bytes);
            DecodedValue::Integer(i64::from_be_bytes(buf))
        }
        6 => {
            buf.copy_from_slice(bytes);
            DecodedValue::Integer(i64::from_be_bytes(buf))
        }
        7 => {
            let float = f64::from_be_bytes(bytes.try_into().unwrap());
            DecodedValue::Float(float)
        }
        8 => DecodedValue::Integer(0),
        9 => DecodedValue::Integer(1),
        12 => DecodedValue::Blob(bytes),
        13 => {
            let text = str::from_utf8(bytes).expect("Error while parsing string from the bytes");
            DecodedValue::Text(text)
        }
        _ => unreachable!(),
    }
}

/// Turn a decoded field into a value that borrows from the same bytes.
pub(crate) fn into_borrowed<'a>(value: DecodedValue<'a>) -> Value<'a> {
    match value {
        DecodedValue::Null => Value::Null,
        DecodedValue::Integer(v) => Value::Integer(v),
        DecodedValue::Float(v) => Value::Float(v),
        DecodedValue::Blob(v) => Value::Blob(Cow::Borrowed(v)),
        DecodedValue::Text(v) => Value::Text(Cow::Borrowed(v)),
    }
}

/// Turn a decoded field into a value that keeps its own copy of the text or
/// blob.
pub(crate) fn into_owned(value: DecodedValue<'_>) -> Value<'static> {
    match value {
        DecodedValue::Null => Value::Null,
        DecodedValue::Integer(v) => Value::Integer(v),
        DecodedValue::Float(v) => Value::Float(v),
        DecodedValue::Blob(v) => Value::Blob(Cow::Owned(v.to_owned())),
        DecodedValue::Text(v) => Value::Text(Cow::Owned(v.to_owned())),
    }
}

impl Tuple {
    /// Serialize a record into bytes: a header containing the serial types, followed
    /// by the payload those types describe.
    ///
    /// A record contains a header and a body, in that order. The header starts with
    /// a varint containing its total size in bytes, including the size varint itself.
    /// This is followed by one serial type varint for each value, describing its
    /// data type.
    ///
    /// The header size is only known after all serial types have been written, so
    /// they are first written into the reserved space. Once the size is known, its
    /// varint is written at the beginning, the serial types are moved into place,
    /// and the payload is appended afterward.
    ///
    /// # Example
    ///
    /// Lets try to encode the following record:
    /**

    id   = 1
    name = "Alice"
    age  = 22

    Therefore, the record looks like:
    +-----+-------+------+
    |  1  | Alice |  22  |
    +-----+-------+------+
    We start by encoding the SQL types. Let's process each value one by one.
    First value: id = 1
    The value 1 fits in a single byte, so the SQL type is `SERIAL_INT8`.
    Therefore:

    serial type = 1 decimal = 01 hex
    value       = 1 decimal = 01 hex

    So, the header so far is [01], and the payload is [01].
    The next value is "Alice".
    The SQLite formula for a TEXT value is:
        13 + 2 * LEN
    "Alice" is 5 bytes long, so:
        13 + 2 * 5 = 23
    Therefore, the serial type is 23.
    Later, when decoding the value, we can recover the length by
    inverting the formula.
    We have:
        13 + 2 * LEN = 23
    We are looking for LEN, so we solve this first degree equation:
        2 * LEN = 23 - 13
        2 * LEN = 10
        LEN = 10 / 2
        LEN = 5
    This is correct because `"Alice".len()` is 5.
    So we found that the encoded serial type is 23:
        serial type = 23 decimal = 17 hex
    The value bytes are:
    +-----+------+------+------+------+
    | 41  |  6c  |  69  |  63  |  65  |
    +-----+------+------+------+------+
    |  A  |  l   |  i   |  c   |  e   |
    +-----+------+------+------+------+

    Therefore, the header so far is [01, 17], and the payload is:
        [01, 41, 6c, 69, 63, 65]
    The last value is age = 22.
    The value 22 fits in a single byte, so its serial type is `SERIAL_INT8`.
    Therefore:
        serial type = 1 decimal = 01 hex
        value       = 22 decimal = 16 hex
    Therefore:
        Header:  [01, 17, 01]
        Payload: [01, 41, 6c, 69, 63, 65, 16]
    Now we need to add the header size so that the decoder knows
    how many bytes belong to the header.
    Handling the header length is a special case because the length
    itself must be included in the header length.
    The header currently contains 3 bytes:
        [01, 17, 01]

    We first encode the header length (3) as a varint.
    Since 3 fits in a single byte, the encoded varint is one byte:
        header length = 3
        header length varint size = 1 byte
    The header length must include the byte used to store the
    header length itself.
    Therefore:
        1 + 3 = 4
    So the final header length is 4.
    We then move the type bytes to make room for the header length
    byte. The header becomes:
        [04, 01, 17, 01]
    But why do we need this?
    Without the header length, the decoder would not know where
    the header ends and where the payload begins.
    Now the first byte tells us that the header is 4 bytes long,
    even though there are only 3 serial type bytes.
    During decoding, `MemCursor`'s `read_varint` [`function`](`crate::MemCursor::read_next_varint`)
    returns both the decoded value and the number of bytes consumed.
    In our case:

        (val = 4, consumed = 1)
    `val` tells us that the total header size is 4 bytes.
    `consumed` tells us that 1 byte was used to encode that value.
    Therefore, the number of serial type bytes is:
        val - consumed
        4 - 1 = 3
    So there are 3 serial types to read.
    We keep reading serial types until the total number of consumed
    header bytes reaches the header size.

    The output looks like

    +------------+------+------+------+------+-----+------+------+------+------+-----+
    |     04     |  01  |  17  |  01  |  01  | 41  |  6c  |  69  |  63  |  65  | 16  |
    +------------+------+------+------+------+-----+------+------+------+------+-----+
    |            |                           |                                       |
    +-Header-len-+-------Data types----------+---------------Payload-----------------+
    */
    // Well... 120 lines of explanation for a 20 line function.
    pub fn serialize(values: &[Value]) -> Vec<u8> {
        let mut payload = Vec::new();
        let mut header = vec![0u8; 9];
        let mut buffer = [0u8; 9];

        for value in values {
            let data_type = Tuple::encode_sqltype(value, &mut payload);
            let vint = encode_varint(&mut buffer, data_type as _);
            header.extend_from_slice(&buffer[..vint]);
        }

        let header_len = header.len() - 9;

        let len = encode_varint(&mut buffer, header_len as _);
        let with_len = encode_varint(&mut buffer, len as u64 + header_len as u64);

        header[0..with_len].copy_from_slice(&buffer[..with_len]);
        header.copy_within(9.., with_len);
        header.truncate(with_len + header_len);
        header.extend_from_slice(&payload);

        header
    }
}

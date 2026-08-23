use super::*;

pub struct Tuple;
impl Tuple {
    pub fn content_size(serial_type: u64) -> RecordMetadata {
        match serial_type {
            0 => RM::new(SERIAL_NULL, 0),
            1 => RM::new(SERIAL_INT8, 1),
            2 => RM::new(SERIAL_INT16, 2),
            3 => RM::new(SERIAL_INT24, 3),
            4 => RM::new(SERIAL_INT32, 4),
            5 => RM::new(SERIAL_INT48, 6),
            6 => RM::new(SERIAL_INT64, 8),
            7 => RM::new(SERIAL_FLOAT64, 8),
            8 => RM::new(SERIAL_INT0, 0),
            9 => RM::new(SERIAL_INT1, 0),

            // 10 and 11 are reserved.
            10 | 11 => unreachable!(),

            // BLOB
            n if n >= 12 && n % 2 == 0 => RM::new(SERIAL_BLOB_MIN, ((n - 12) / 2) as usize),

            // TEXT
            n if n >= 13 && n % 2 == 1 => RM::new(SERIAL_TEXT_MIN, ((n - 13) / 2) as usize),

            _ => unreachable!(),
        }
    }

    pub fn decode_sqltype_borrowed<'a>(bytes: &'a [u8], record_metadata: &RM) -> Value<'a> {
        let mut buf = [0u8; 8];
        match record_metadata.serial_type {
            0 => Value::Null,
            1 => {
                buf[7..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            2 => {
                buf[6..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            3 => {
                buf[5..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            4 => {
                buf[4..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            5 => {
                buf[2..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            6 => {
                buf.copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            7 => {
                let float = f64::from_be_bytes(bytes.try_into().unwrap());
                Value::Float(float)
            }
            8 => Value::Integer(0),
            9 => Value::Integer(1),
            12 => Value::Blob(Cow::Borrowed(bytes)),
            13 => {
                let text =
                    str::from_utf8(bytes).expect("Error while parsing string from the bytes");

                Value::Text(Cow::Borrowed(text))
            }
            _ => unreachable!(),
        }
    }
    pub fn decode_sqltype_owned(bytes: &[u8], record_metadata: &RM) -> Value<'static> {
        let mut buf = [0u8; 8];
        match record_metadata.serial_type {
            0 => Value::Null,
            1 => {
                buf[7..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            2 => {
                buf[6..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            3 => {
                buf[5..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            4 => {
                buf[4..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            5 => {
                buf[2..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            6 => {
                buf.copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                Value::Integer(int)
            }
            7 => {
                let float = f64::from_be_bytes(bytes.try_into().unwrap());
                Value::Float(float)
            }
            8 => Value::Integer(0),
            9 => Value::Integer(1),
            12 => Value::Blob(Cow::Owned(bytes.to_owned())),
            13 => {
                let text =
                    str::from_utf8(bytes).expect("Error while parsing string from the bytes");
                Value::Text(Cow::Owned(text.to_owned()))
            }
            _ => unreachable!(),
        }
    }

    pub fn encode_sqltype(value: &Value, output: &mut Vec<u8>) -> usize {
        match value {
            Value::Integer(n) => {
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
            Value::Float(f) => {
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

const fn text_encoding(len: usize) -> usize {
    (len * 2) + 13
}

const fn blob_encoding(len: usize) -> usize {
    (len * 2) + 12
}

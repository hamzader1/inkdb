pub const SERIAL_NULL: u8 = 0;
pub const SERIAL_INT8: u8 = 1;
pub const SERIAL_INT16: u8 = 2;
pub const SERIAL_INT24: u8 = 3;
pub const SERIAL_INT32: u8 = 4;
pub const SERIAL_INT48: u8 = 5;
pub const SERIAL_INT64: u8 = 6;
pub const SERIAL_FLOAT64: u8 = 7;
pub const SERIAL_INT0: u8 = 8;
pub const SERIAL_INT1: u8 = 9;
// Reserved by Sqlite
/*
   const SERIAL_RESERVED_10: u8 = 10;
   const SERIAL_RESERVED_11: u8 = 11;
*/
pub const SERIAL_BLOB_MIN: u8 = 12;
pub const SERIAL_TEXT_MIN: u8 = 13;

// pub const SIZE_NULL: usize = 0;
// pub const SIZE_INT8: usize = 1;
// pub const SIZE_INT16: usize = 2;
// pub const SIZE_INT24: usize = 3;
// pub const SIZE_INT32: usize = 4;
// pub const SIZE_INT48: usize = 6;
// pub const SIZE_INT64: usize = 8;
// pub const SIZE_FLOAT64: usize = 8;
// pub const SIZE_INT0: usize = 0;

// EXPERIMENTAL
pub trait SqlType {
    fn convert<'a>(self) -> Value<'a>;
}
#[macro_export]
macro_rules! impl_ints {
    ($($t:ty),*) => {
        $(impl SqlType for $t {
            fn convert<'a>(self) -> Value<'a> {
                Value::Integer(self as i64)
            }
        })*
    };
}
impl_ints!(i8, i16, i32, i64, u8, u16, u32, u64);

use std::cmp::Ordering;

#[derive(Debug)]
pub enum Value<'a> {
    Null,
    Integer(i64),
    Real(f64),
    Text(&'a str),
    Blob(&'a [u8]),
}

pub struct RecordMetadata {
    col_type: u8,
    col_size: usize,
}

impl RecordMetadata {
    fn new(col_type: u8, col_size: usize) -> Self {
        Self { col_type, col_size }
    }
}
pub type RM = RecordMetadata;

impl<'a> PartialEq for Value<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<'a> Eq for Value<'a> {}

impl<'a> PartialOrd for Value<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for Value<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Value::Null, Value::Null) => Ordering::Equal,

            (Value::Integer(a), Value::Integer(b)) => a.cmp(b),

            (Value::Real(a), Value::Real(b)) => a.total_cmp(b),

            (Value::Text(a), Value::Text(b)) => a.cmp(b),

            (Value::Blob(a), Value::Blob(b)) => a.cmp(b),

            _ => panic!(
                "Comparing {} to {} is not allowed",
                std::any::type_name_of_val(self),
                std::any::type_name_of_val(other)
            ),
        }
    }
}

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

    pub fn decode_sqltype<'a>(bytes: &'a [u8], record_metadata: &RM) -> Value<'a> {
        let mut buf = [0u8; 8];
        match record_metadata.serial_type {
            0 => return Value::Null,
            1 => {
                buf[7..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                return Value::Integer(int);
            }
            2 => {
                buf[6..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                return Value::Integer(int);
            }
            3 => {
                buf[5..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                return Value::Integer(int);
            }
            4 => {
                buf[4..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                return Value::Integer(int);
            }
            5 => {
                buf[2..8].copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                return Value::Integer(int);
            }
            6 => {
                buf.copy_from_slice(bytes);
                let int = i64::from_be_bytes(buf);
                return Value::Integer(int);
            }
            7 => {
                let float = f64::from_be_bytes(bytes.try_into().unwrap());
                return Value::Real(float);
            }
            8 => return Value::Integer(0),
            9 => return Value::Integer(1),
            12 => return Value::Blob(bytes),
            13 => {
                let text =
                    str::from_utf8(bytes).expect("Error while parsing string from the bytes");
                return Value::Text(text);
            }
            _ => unreachable!(),
        }
    }
}

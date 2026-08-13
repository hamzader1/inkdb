use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::hash_map::VacantEntry;

use crate::errors::SqliteError;
use std::ops::{Add, Div, Mul, Sub};

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

const MAX_SAFE_INT: i64 = 9_007_199_254_740_992; //  2^53
const MIN_SAFE_INT: i64 = -9_007_199_254_740_992; // -2^53

// EXPERIMENTAL
pub trait SqlType {
    fn into_sqlite_value<'a>(self) -> Value<'a>;
}
#[macro_export]
macro_rules! impl_ints {
    ($($t:ty),*) => {
        $(impl SqlType for $t {
            fn into_sqlite_value<'a>(self) -> Value<'a> {
                Value::Integer(self as i64)
            }
        })*
    };
}
impl_ints!(i8, i16, i32, i64, u8, u16, u32, u64);
#[derive(Debug)]
pub struct RecordMetadata {
    pub serial_type: u8,
    pub size: usize,
}

impl RecordMetadata {
    fn new(serial_type: u8, size: usize) -> Self {
        Self { serial_type, size }
    }
}
pub type RM = RecordMetadata;

#[derive(Debug)]
pub enum Value<'a> {
    Null,
    Integer(i64),
    Float(f64),
    Text(Cow<'a, str>),
    Blob(Cow<'a, [u8]>),
    Tuple(Vec<Value<'a>>),
}
impl<'a> Value<'a> {
    pub fn into_owned(&self) -> Value<'static> {
        match self {
            Value::Text(x) => Value::Text(Cow::Owned(x.as_ref().to_string())),
            Value::Blob(x) => Value::Blob(Cow::Owned(x.as_ref().to_owned())),
            Value::Float(f) => Value::Float(*f),
            Value::Integer(n) => Value::Integer(*n),
            Value::Null => Value::Null,
            Value::Tuple(t) => Value::Tuple(t.iter().map(|inner| inner.into_owned()).collect()),
        }
    }
}

impl<'a> Value<'a> {
    pub fn text(txt: &str) -> Self {
        Self::Text(Cow::Owned(txt.into()))
    }
}
impl<'a> Eq for Value<'a> {}

impl<'a, 'b> PartialEq<Value<'b>> for Value<'a> {
    fn eq(&self, other: &Value<'b>) -> bool {
        compare_values(self, other) == Ordering::Equal
    }
}

impl<'a, 'b> PartialOrd<Value<'b>> for Value<'a> {
    fn partial_cmp(&self, other: &Value<'b>) -> Option<Ordering> {
        Some(compare_values(self, other))
    }
}

impl<'a> Ord for Value<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_values(self, other)
    }
}

fn compare_values(a: &Value<'_>, b: &Value<'_>) -> Ordering {
    match (a, b) {
        // NULL
        (Value::Null, Value::Null) => Ordering::Equal,

        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,

        // INTEGER / REAL
        (Value::Integer(a), Value::Integer(b)) => a.cmp(b),

        (Value::Float(a), Value::Float(b)) => a.total_cmp(b),

        (Value::Integer(a), Value::Float(b)) => compare_sqlite_num(*a, *b),

        (Value::Float(a), Value::Integer(b)) => compare_sqlite_num(*b, *a).reverse(),

        // Numeric < TEXT
        (Value::Integer(_), Value::Text(_)) | (Value::Float(_), Value::Text(_)) => Ordering::Less,

        (Value::Text(_), Value::Integer(_)) | (Value::Text(_), Value::Float(_)) => {
            Ordering::Greater
        }

        // Numeric < BLOB
        (Value::Integer(_), Value::Blob(_)) | (Value::Float(_), Value::Blob(_)) => Ordering::Less,

        (Value::Blob(_), Value::Integer(_)) | (Value::Blob(_), Value::Float(_)) => {
            Ordering::Greater
        }

        // TEXT
        (Value::Text(a), Value::Text(b)) => a.cmp(b),

        // TEXT < BLOB
        (Value::Text(_), Value::Blob(_)) => Ordering::Less,

        (Value::Blob(_), Value::Text(_)) => Ordering::Greater,

        // BLOB
        (Value::Blob(a), Value::Blob(b)) => a.cmp(b),

        // Everything else < TUPLE
        (
            Value::Null | Value::Integer(_) | Value::Float(_) | Value::Text(_) | Value::Blob(_),
            Value::Tuple(_),
        ) => Ordering::Less,

        // TUPLE > everything else
        (
            Value::Tuple(_),
            Value::Null | Value::Integer(_) | Value::Float(_) | Value::Text(_) | Value::Blob(_),
        ) => Ordering::Greater,

        // TUPLE <=> TUPLE
        (Value::Tuple(a), Value::Tuple(b)) => {
            for (a, b) in a.iter().zip(b.iter()) {
                match compare_values(a, b) {
                    Ordering::Equal => continue,
                    ordering => return ordering,
                }
            }

            a.len().cmp(&b.len())
        }
    }
}

impl<'a> PartialEq<Value<'a>> for i64 {
    fn eq(&self, other: &Value<'a>) -> bool {
        compare_i64_value(*self, other) == Ordering::Equal
    }
}

impl<'a> PartialOrd<Value<'a>> for i64 {
    fn partial_cmp(&self, other: &Value<'a>) -> Option<Ordering> {
        Some(compare_i64_value(*self, other))
    }
}

impl<'a> PartialEq<i64> for Value<'a> {
    fn eq(&self, other: &i64) -> bool {
        compare_i64_value(*other, self) == Ordering::Equal
    }
}

impl<'a> PartialOrd<i64> for Value<'a> {
    fn partial_cmp(&self, other: &i64) -> Option<Ordering> {
        Some(compare_i64_value(*other, self).reverse())
    }
}

fn compare_i64_value(a: i64, b: &Value<'_>) -> Ordering {
    match b {
        Value::Null => Ordering::Greater,

        Value::Integer(v) => a.cmp(v),

        Value::Float(v) => compare_sqlite_num(a, *v),

        Value::Text(_) => Ordering::Less,

        Value::Blob(_) => Ordering::Less,

        Value::Tuple(_) => Ordering::Less,
    }
}

impl<'a> PartialEq<Value<'a>> for f64 {
    fn eq(&self, other: &Value<'a>) -> bool {
        compare_f64_value(*self, other) == Ordering::Equal
    }
}

impl<'a> PartialOrd<Value<'a>> for f64 {
    fn partial_cmp(&self, other: &Value<'a>) -> Option<Ordering> {
        Some(compare_f64_value(*self, other))
    }
}

impl<'a> PartialEq<f64> for Value<'a> {
    fn eq(&self, other: &f64) -> bool {
        compare_f64_value(*other, self) == Ordering::Equal
    }
}

impl<'a> PartialOrd<f64> for Value<'a> {
    fn partial_cmp(&self, other: &f64) -> Option<Ordering> {
        Some(compare_f64_value(*other, self).reverse())
    }
}

fn compare_f64_value(a: f64, b: &Value<'_>) -> Ordering {
    match b {
        Value::Null => Ordering::Greater,

        Value::Integer(v) => compare_sqlite_num(*v, a).reverse(),

        Value::Float(v) => a.total_cmp(v),

        Value::Text(_) => Ordering::Less,

        Value::Blob(_) => Ordering::Less,

        Value::Tuple(_) => Ordering::Less,
    }
}

impl<'a, 'b> PartialEq<Value<'a>> for &'b str {
    fn eq(&self, other: &Value<'a>) -> bool {
        compare_str_value(self, other) == Ordering::Equal
    }
}

impl<'a, 'b> PartialOrd<Value<'a>> for &'b str {
    fn partial_cmp(&self, other: &Value<'a>) -> Option<Ordering> {
        Some(compare_str_value(self, other))
    }
}

impl<'a, 'b> PartialEq<&'b str> for Value<'a> {
    fn eq(&self, other: &&'b str) -> bool {
        compare_str_value(other, self) == Ordering::Equal
    }
}

impl<'a, 'b> PartialOrd<&'b str> for Value<'a> {
    fn partial_cmp(&self, other: &&'b str) -> Option<Ordering> {
        Some(compare_str_value(other, self).reverse())
    }
}

fn compare_str_value(a: &str, b: &Value<'_>) -> Ordering {
    match b {
        Value::Null => Ordering::Greater,

        Value::Integer(_) | Value::Float(_) => Ordering::Greater,

        Value::Text(v) => a.cmp(v.as_ref()),

        Value::Blob(_) => Ordering::Less,

        Value::Tuple(_) => Ordering::Less,
    }
}

impl<'a> PartialEq<Value<'a>> for String {
    fn eq(&self, other: &Value<'a>) -> bool {
        compare_str_value(self.as_str(), other) == Ordering::Equal
    }
}

impl<'a> PartialOrd<Value<'a>> for String {
    fn partial_cmp(&self, other: &Value<'a>) -> Option<Ordering> {
        Some(compare_str_value(self.as_str(), other))
    }
}

impl<'a> PartialEq<String> for Value<'a> {
    fn eq(&self, other: &String) -> bool {
        compare_str_value(other.as_str(), self) == Ordering::Equal
    }
}

impl<'a> PartialOrd<String> for Value<'a> {
    fn partial_cmp(&self, other: &String) -> Option<Ordering> {
        Some(compare_str_value(other.as_str(), self).reverse())
    }
}

impl<'a, 'b> PartialEq<Value<'a>> for &'b [u8] {
    fn eq(&self, other: &Value<'a>) -> bool {
        compare_bytes_value(self, other) == Ordering::Equal
    }
}

impl<'a, 'b> PartialOrd<Value<'a>> for &'b [u8] {
    fn partial_cmp(&self, other: &Value<'a>) -> Option<Ordering> {
        Some(compare_bytes_value(self, other))
    }
}

impl<'a, 'b> PartialEq<&'b [u8]> for Value<'a> {
    fn eq(&self, other: &&'b [u8]) -> bool {
        compare_bytes_value(*other, self) == Ordering::Equal
    }
}

impl<'a, 'b> PartialOrd<&'b [u8]> for Value<'a> {
    fn partial_cmp(&self, other: &&'b [u8]) -> Option<Ordering> {
        Some(compare_bytes_value(*other, self).reverse())
    }
}

fn compare_bytes_value(a: &[u8], b: &Value<'_>) -> Ordering {
    match b {
        Value::Null => Ordering::Greater,

        Value::Integer(_) | Value::Float(_) => Ordering::Greater,

        Value::Text(_) => Ordering::Greater,

        Value::Blob(v) => a.cmp(v.as_ref()),

        Value::Tuple(_) => Ordering::Less,
    }
}

impl<'a> PartialEq<Value<'a>> for Vec<u8> {
    fn eq(&self, other: &Value<'a>) -> bool {
        compare_bytes_value(self, other) == Ordering::Equal
    }
}

impl<'a> PartialOrd<Value<'a>> for Vec<u8> {
    fn partial_cmp(&self, other: &Value<'a>) -> Option<Ordering> {
        Some(compare_bytes_value(self, other))
    }
}

impl<'a> PartialEq<Vec<u8>> for Value<'a> {
    fn eq(&self, other: &Vec<u8>) -> bool {
        compare_bytes_value(other, self) == Ordering::Equal
    }
}

impl<'a> PartialOrd<Vec<u8>> for Value<'a> {
    fn partial_cmp(&self, other: &Vec<u8>) -> Option<Ordering> {
        Some(compare_bytes_value(other, self).reverse())
    }
}

/*
   SQLite serial type codes:
   0       -> NULL
   1       -> i8
   2       -> BE i16
   3       -> BE i24
   4       -> BE i32
   5       -> BE i48
   6       -> BE i64
   7       -> BE f64 (IEEE 754)
   8       -> integer 0
   9       -> integer 1
   10,11    -> reserved/internal
   N>=12 even -> BLOB, (N-12)/2 bytes
   N>=13 odd  -> TEXT, (N-13)/2 bytes
*/
pub struct Record;
impl Record {
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
}

pub fn compare_sqlite_num(i: i64, f: f64) -> Ordering {
    // Safe Window Optimization: If the integer safely fits in 53 bits,
    // casting to f64 is mathematically lossless.
    if (MIN_SAFE_INT..=MAX_SAFE_INT).contains(&i) {
        return (i as f64).total_cmp(&f);
    }

    // Beyond the Safe Window: The integer requires real 64-bit accuracy.
    // We check if the float falls outside the physical numeric bounds of an i64.
    // Note: We use strict boundaries to bypass casting edge cases at 2^63 - 1.
    if f >= 9223372036854775808.0 {
        // 2^63
        return Ordering::Less; // i is completely smaller than f
    }
    if f < -9223372036854775808.0 {
        // -2^63
        return Ordering::Greater; // i is completely larger than f
    }

    // Safe Float to Int Downcast: Since the float falls inside the i64 boundary,
    // we can safely truncate its fraction to extract the whole number.
    let f_as_i64 = f as i64;

    match i.cmp(&f_as_i64) {
        Ordering::Equal => {
            // Tie breaker: The integer matches the truncated float whole number part.
            // We calculate the remaining decimal fraction on the float side.
            let fraction = f - (f_as_i64 as f64);
            if fraction > 0.0 {
                Ordering::Less
            } else if fraction < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal // Perfect arithmetic match
            }
        }
        any => any,
    }
}

impl<'a> Value<'a> {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "NULL",
            Value::Integer(_) => "INTEGER",
            Value::Float(_) => "REAL",
            Value::Text(_) => "TEXT",
            Value::Blob(_) => "BLOB",
            Value::Tuple(_) => "TUPLE",
        }
    }
    pub fn to_string(&self) -> Result<String, SqliteError> {
        match self {
            Value::Null => Ok("NULL".to_string()),
            Value::Integer(n) => Ok(n.to_string()),
            Value::Float(n) => Ok(n.to_string()),
            Value::Text(txt) => Ok(txt.to_string()),
            Value::Blob(_) => Err(SqliteError::TypeMismatch {
                expected: "TEXT",
                actual: self.type_name(),
            }),

            Value::Tuple(_) => Err(SqliteError::TypeMismatch {
                expected: "TEXT",
                actual: self.type_name(),
            }),
        }
    }
    pub fn get_int(&self) -> Result<i64, SqliteError> {
        match self {
            Value::Integer(n) => Ok(*n),
            other => Err(SqliteError::TypeMismatch {
                expected: "INTEGER",
                actual: other.type_name(),
            }),
        }
    }
    pub fn get_float(&self) -> Result<f64, SqliteError> {
        match self {
            Value::Float(n) => Ok(*n),
            Value::Integer(n) => Ok(*n as f64),
            other => Err(SqliteError::TypeMismatch {
                expected: "REAL",
                actual: other.type_name(),
            }),
        }
    }
}

impl<'a> Add for Value<'a> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::Integer(a), Self::Integer(b)) => Self::Integer(a + b),
            (Self::Integer(a), Self::Float(b)) => Self::Float(a as f64 + b),
            (Self::Float(a), Self::Integer(b)) => Self::Float(a + b as f64),
            (Self::Float(a), Self::Float(b)) => Self::Float(a + b),

            (Self::Null, _) | (_, Self::Null) => Self::Null,

            (a, b) => panic!("cannot add {a:?} and {b:?}"),
        }
    }
}

impl<'a> Sub for Value<'a> {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::Integer(a), Self::Integer(b)) => Self::Integer(a - b),
            (Self::Integer(a), Self::Float(b)) => Self::Float(a as f64 - b),
            (Self::Float(a), Self::Integer(b)) => Self::Float(a - b as f64),
            (Self::Float(a), Self::Float(b)) => Self::Float(a - b),

            (Self::Null, _) | (_, Self::Null) => Self::Null,

            (a, b) => panic!("cannot subtract {b:?} from {a:?}"),
        }
    }
}

impl<'a> Mul for Value<'a> {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::Integer(a), Self::Integer(b)) => Self::Integer(a * b),
            (Self::Integer(a), Self::Float(b)) => Self::Float(a as f64 * b),
            (Self::Float(a), Self::Integer(b)) => Self::Float(a * b as f64),
            (Self::Float(a), Self::Float(b)) => Self::Float(a * b),

            (Self::Null, _) | (_, Self::Null) => Self::Null,

            (a, b) => panic!("cannot multiply {a:?} and {b:?}"),
        }
    }
}

impl<'a> Div for Value<'a> {
    type Output = Self;

    fn div(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::Integer(a), Self::Integer(b)) => {
                if b == 0 {
                    panic!("division by zero");
                }

                Self::Integer(a / b)
            }

            (Self::Integer(a), Self::Float(b)) => Self::Float(a as f64 / b),
            (Self::Float(a), Self::Integer(b)) => Self::Float(a / b as f64),
            (Self::Float(a), Self::Float(b)) => Self::Float(a / b),

            (Self::Null, _) | (_, Self::Null) => Self::Null,

            (a, b) => panic!("cannot divide {a:?} by {b:?}"),
        }
    }
}

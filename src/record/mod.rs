pub mod cmp;
pub mod tuple;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::hash_map::VacantEntry;

use crate::errors::SqliteError;
use std::ops::{Add, Div, Mul, Sub};
#[rustfmt::skip]
pub const I8_MASK:  i64 = 0x0000_0000_0000_007F;
pub const I16_MASK: i64 = 0x0000_0000_0000_7FFF;
pub const I32_MASK: i64 = 0x0000_0000_7FFF_FFFF;
pub const I64_MASK: i64 = 0x7FFF_FFFF_FFFF_FFFF;

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

#[derive(Debug, Clone)]
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
            Value::Blob(_) => Err(SqliteError::TypeConversionMismatch {
                expected: "TEXT",
                actual: self.type_name(),
            }),

            Value::Tuple(_) => Err(SqliteError::TypeConversionMismatch {
                expected: "TEXT",
                actual: self.type_name(),
            }),
        }
    }
    pub fn get_int(&self) -> Result<i64, SqliteError> {
        match self {
            Value::Integer(n) => Ok(*n),
            other => Err(SqliteError::TypeConversionMismatch {
                expected: "INTEGER",
                actual: other.type_name(),
            }),
        }
    }
    pub fn get_float(&self) -> Result<f64, SqliteError> {
        match self {
            Value::Float(n) => Ok(*n),
            Value::Integer(n) => Ok(*n as f64),
            other => Err(SqliteError::TypeConversionMismatch {
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
                    return Self::Integer(0);
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

impl<'a> std::fmt::Display for Value<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Null => write!(f, "Null"),
            Value::Integer(int) => write!(f, "{}", int),
            &Value::Float(fl) => write!(f, "{}", fl),
            Value::Text(t) => write!(f, "{}", t),
            Value::Blob(b) => {
                write!(f, "[")?;
                for (i, val) in b.iter().enumerate() {
                    write!(f, "{}", val)?;

                    if i < b.len() - 1 {
                        write!(f, ", ")?;
                    }
                }
                write!(f, "]")?;
                Ok(())
            }
            Value::Tuple(b) => {
                for (i, val) in b.iter().enumerate() {
                    write!(f, "{}", val)?;

                    if i < b.len() - 1 {
                        write!(f, ", ")?;
                    }
                }
                Ok(())
            }
        }
    }
}
impl<'a> Value<'a> {
    pub fn to_bool(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Integer(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Text(_) => true,
            _ => false,
        }
    }
}

#[derive(Debug)]
pub enum CompressedNumeric {
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}
// impl CompressedNumeric {
//     fn into_int<T: Copy + Clone>(self) -> T {
//         match self {
//             Self::I8(x) => {
//                 let v = (&x as *const i8 as *const T);
//                 unsafe { *v }
//             }
//             _ => todo!(),
//         }
//     }
// }
impl<'a> From<&Value<'a>> for CompressedNumeric {
    fn from(value: &Value<'a>) -> Self {
        // let value = value.get_int().unwrap();
        match value.get_int() {
            Ok(value) => {
                if value & I8_MASK == value {
                    Self::I8(value as _)
                } else if value & I16_MASK == value {
                    Self::I16(value as _)
                } else if value & I32_MASK == value {
                    Self::I32(value as _)
                } else {
                    Self::I64(value as _)
                }
            }
            _ => match value.get_float() {
                Ok(value) => {
                    if (value as f32) as f64 == value {
                        Self::F32(value as f32)
                    } else {
                        Self::F64(value)
                    }
                }
                _ => panic!("Compressing works only for integers and floats"),
            },
        }
    }
}

pub mod arith;
pub mod cmp;
/*Temporary for now*/
#[allow(clippy::module_inception)]
pub mod record;
pub mod tuple;

pub(crate) use arith::{TryAdd, TryDiv, TryMul, TrySub};
pub use record::Record;

use std::borrow::Cow;
use std::cmp::Ordering;

use crate::errors::InkError;
/// A mask over the bits an `i8` can hold.
///
/// An integer this mask leaves untouched has no bits above the eighth, so the
/// encoder can spend a single byte on it.
#[rustfmt::skip]
pub const I8_MASK:  i64 = 0x0000_0000_0000_007F;
/// A mask over the bits an `i16` can hold. See [`I8_MASK`].
pub const I16_MASK: i64 = 0x0000_0000_0000_7FFF;
/// A mask over the bits an `i32` can hold. See [`I8_MASK`].
pub const I32_MASK: i64 = 0x0000_0000_7FFF_FFFF;
/// A mask over the bits an `i64` can hold. See [`I8_MASK`].
pub const I64_MASK: i64 = 0x7FFF_FFFF_FFFF_FFFF;

/// Serial type codes, as SQLite defines them.
///
/// Every field of a record is tagged with one of these numbers, and the tag says
/// both what the field holds and how much room it takes. Numbers are written big
/// endian. Zero is NULL, and one through six are signed integers of 8, 16, 24,
/// 32, 48 and 64 bits. Seven is a 64 bit float. Eight and nine are the integers
/// 0 and 1, which take no bytes at all. Ten and eleven are reserved by SQLite.
/// From twelve on the tag carries a length too: an even tag of 12 or more is a
/// blob of (tag - 12) / 2 bytes, and an odd tag of 13 or more is text of
/// (tag - 13) / 2 bytes.
/// More about this: [`source`](https://sqlite.org/fileformat.html#schema_layer)
pub const SERIAL_NULL: u8 = 0;
/// A signed integer in one byte.
pub const SERIAL_INT8: u8 = 1;
/// A signed integer in two bytes.
pub const SERIAL_INT16: u8 = 2;
/// A signed integer in three bytes.
pub const SERIAL_INT24: u8 = 3;
/// A signed integer in four bytes.
pub const SERIAL_INT32: u8 = 4;
/// A signed integer in six bytes.
pub const SERIAL_INT48: u8 = 5;
/// A signed integer in eight bytes.
pub const SERIAL_INT64: u8 = 6;
/// A float in eight bytes.
pub const SERIAL_FLOAT64: u8 = 7;
/// The integer 0, which is written without any bytes.
pub const SERIAL_INT0: u8 = 8;
/// The integer 1, which is written without any bytes.
pub const SERIAL_INT1: u8 = 9;
/// The lowest tag that means a blob. Every even tag from here up does, and the
/// length of the blob follows from the tag.
pub const SERIAL_BLOB_MIN: u8 = 12;
/// The lowest tag that means text. Every odd tag from here up does, and the
/// length of the text follows from the tag.
pub const SERIAL_TEXT_MIN: u8 = 13;

/// The largest whole number a float still holds exactly, so an integer inside
/// this window can be compared with a float by casting and losing nothing.
const MAX_SAFE_INT: i64 = 9_007_199_254_740_992; // 2^53
/// The smallest whole number a float still holds exactly, the other end of the
/// same window.
const MIN_SAFE_INT: i64 = -9_007_199_254_740_992; // -2^53

/// What one field's serial type says about it: which kind of value it is and how
/// many bytes it takes.
#[derive(Debug)]
pub(crate) struct RecordMetadata {
    /// The decoded serial type, one of the `SERIAL_*` codes above.
    pub serial_type: u8,
    /// The length of the field's bytes.
    pub size: usize,
}

impl RecordMetadata {
    fn new(serial_type: u8, size: usize) -> Self {
        Self { serial_type, size }
    }
}

/// One value, the smallest thing a record is made of.
///
/// Text and blobs borrow from the bytes of the record they were read from, and
/// are copied only when the value has to outlive them. A tuple is several values
/// under one key, which is what an index entry is.
#[derive(Debug, Clone)]
pub enum Value<'a> {
    /// A missing or unknown value.
    Null,
    /// A whole number.
    Integer(i64),
    /// A number with a fractional part.
    Float(f64),
    /// A string of characters.
    Text(Cow<'a, str>),
    /// A run of bytes, with no meaning of its own.
    Blob(Cow<'a, [u8]>),
    /// Several values together, in a fixed order.
    Tuple(Box<[Value<'a>]>),
}

impl<'a> Value<'a> {
    /// Take ownership of the value, copying any text or blob it borrows.
    pub fn into_static(self) -> Value<'static> {
        match self {
            Value::Text(x) => Value::Text(Cow::Owned(x.into_owned())),
            Value::Blob(x) => Value::Blob(Cow::Owned(x.into_owned())),
            Value::Tuple(t) => Value::Tuple(t.into_iter().map(Value::into_static).collect()),
            Value::Float(f) => Value::Float(f),
            Value::Integer(n) => Value::Integer(n),
            Value::Null => Value::Null,
        }
    }

    /// A copy that owns its text and blob, leaving the original alone.
    pub fn to_owned_static(&self) -> Value<'static> {
        match self {
            Value::Text(x) => Value::Text(Cow::Owned(x.as_ref().to_string())),
            Value::Blob(x) => Value::Blob(Cow::Owned(x.as_ref().to_owned())),
            Value::Float(f) => Value::Float(*f),
            Value::Integer(n) => Value::Integer(*n),
            Value::Null => Value::Null,
            Value::Tuple(t) => {
                Value::Tuple(t.iter().map(|inner| inner.to_owned_static()).collect())
            }
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

impl From<i64> for Value<'static> {
    fn from(value: i64) -> Self {
        Value::Integer(value)
    }
}
impl From<u64> for Value<'static> {
    fn from(value: u64) -> Self {
        Value::Integer(value as _)
    }
}

impl<'a> From<&'a str> for Value<'static> {
    fn from(value: &'a str) -> Self {
        Self::Text(Cow::Owned(value.into()))
    }
}
impl From<String> for Value<'static> {
    fn from(value: String) -> Self {
        Self::Text(Cow::Owned(value))
    }
}

impl<'a> Value<'a> {
    /// The name of the value's type.
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
    /// Write the value as text.
    ///
    /// Blobs and tuples have no one sensible text form, so they are reported as
    /// a failed conversion rather than guessed at.
    ///
    /// # Errors
    /// [`InkError::type_conversion`] when the value cannot be written as text.
    pub fn to_string(&self) -> Result<String, InkError> {
        match self {
            Value::Null => Ok("NULL".to_string()),
            Value::Integer(n) => Ok(n.to_string()),
            Value::Float(n) => Ok(n.to_string()),
            Value::Text(txt) => Ok(txt.to_string()),
            Value::Blob(_) => Err(InkError::type_conversion("TEXT", self.type_name())),

            Value::Tuple(_) => Err(InkError::type_conversion("TEXT", self.type_name())),
        }
    }
    /// Read the value as a whole number.
    ///
    /// Nothing is converted here: a float or some text is a conversion error
    /// rather than being rounded or parsed.
    ///
    ///
    /// # Errors
    /// [`InkError::type_conversion`] when the value is not an integer.
    ///
    /*  We always call this with a valid Value::Integer. */
    pub fn cast_int(&self) -> Result<i64, InkError> {
        match self {
            Value::Integer(n) => Ok(*n),
            other => Err(InkError::type_conversion("INTEGER", other.type_name())),
        }
    }
    /// Read the value as a float, which an integer also answers to.
    ///
    /// # Errors
    /// [`InkError::type_conversion`] when the value is neither a float nor an
    /// integer.
    pub fn get_float(&self) -> Result<f64, InkError> {
        match self {
            Value::Float(n) => Ok(*n),
            Value::Integer(n) => Ok(*n as f64),
            other => Err(InkError::type_conversion("REAL", other.type_name())),
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
            Value::Blob(b) => write!(f, "{}", String::from_utf8_lossy(b)),
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
    /// NULL, zero and zero point zero are false, and so are blobs and tuples.
    /// Any text is true, even text that reads like a number.
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

/// A number on its way to being written, at the narrowest width that still
/// holds it.
///
/// The masks above decide how many bytes an integer needs. A float is narrowed
/// to `f32` only when that conversion changes nothing, since a record cannot
/// afford to lose precision just to save four bytes.
#[derive(Debug)]
pub(crate) enum CompressedNumeric {
    /// Fits in one byte.
    I8(i8),
    /// Fits in two bytes.
    I16(i16),
    /// Fits in four bytes.
    I32(i32),
    /// Needs all eight bytes.
    I64(i64),
    /// A float that survives being narrowed to `f32`.
    F32(f32),
    /// A float that needs all eight bytes.
    F64(f64),
}

impl<'a> From<&Value<'a>> for CompressedNumeric {
    fn from(value: &Value<'a>) -> Self {
        match value.cast_int() {
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
                // Must be unreachable by this time.
                _ => panic!("Compressing works only for integers and floats"),
            },
        }
    }
}

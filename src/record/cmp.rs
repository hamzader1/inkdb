use super::*;

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

impl<'a> PartialEq<Value<'a>> for &str {
    fn eq(&self, other: &Value<'a>) -> bool {
        compare_str_value(self, other) == Ordering::Equal
    }
}

impl<'a> PartialOrd<Value<'a>> for &str {
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

impl<'a> PartialEq<Value<'a>> for &[u8] {
    fn eq(&self, other: &Value<'a>) -> bool {
        compare_bytes_value(self, other) == Ordering::Equal
    }
}

impl<'a> PartialOrd<Value<'a>> for &[u8] {
    fn partial_cmp(&self, other: &Value<'a>) -> Option<Ordering> {
        Some(compare_bytes_value(self, other))
    }
}

impl<'a> PartialEq<&[u8]> for Value<'a> {
    fn eq(&self, other: &&[u8]) -> bool {
        compare_bytes_value(other, self) == Ordering::Equal
    }
}

impl<'a> PartialOrd<&[u8]> for Value<'a> {
    fn partial_cmp(&self, other: &&[u8]) -> Option<Ordering> {
        Some(compare_bytes_value(other, self).reverse())
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

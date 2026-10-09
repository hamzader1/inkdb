use super::*;

impl<'a> Eq for Value<'a> {}

/// Two values are equal exactly when the ordering finds nothing between them.
impl<'a, 'b> PartialEq<Value<'b>> for Value<'a> {
    fn eq(&self, other: &Value<'b>) -> bool {
        compare_values(self, other) == Ordering::Equal
    }
}

/// Every pair of values is comparable, so this never answers `None`.
impl<'a, 'b> PartialOrd<Value<'b>> for Value<'a> {
    fn partial_cmp(&self, other: &Value<'b>) -> Option<Ordering> {
        Some(compare_values(self, other))
    }
}

/// The order that sorting and index seeks use. It is decided in one place, so
/// `==`, `<` and a sort can never disagree.
impl<'a> Ord for Value<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_values(self, other)
    }
}

/// Order two values the way SQLite does.
///
/// The kinds of value are ranked NULL first, then numbers, then text, then
/// blobs, then tuples, and inside a kind the natural order takes over. Numbers
/// compare exactly, without the rounding a cast to a float would bring, which is
/// why integer against float is handled by [`compare_num`]. A tuple compares
/// element by element and, when one is a prefix of the other, the shorter one
/// comes first.
fn compare_values(a: &Value<'_>, b: &Value<'_>) -> Ordering {
    match (a, b) {
        // NULL
        (Value::Null, Value::Null) => Ordering::Equal,

        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,

        // INTEGER / REAL
        (Value::Integer(a), Value::Integer(b)) => a.cmp(b),

        (Value::Float(a), Value::Float(b)) => a.total_cmp(b),

        (Value::Integer(a), Value::Float(b)) => compare_num(*a, *b),

        (Value::Float(a), Value::Integer(b)) => compare_num(*b, *a).reverse(),

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
            Value::Integer(_) | Value::Float(_) | Value::Text(_) | Value::Blob(_),
            Value::Tuple(_),
        ) => Ordering::Less,

        // TUPLE > everything else
        (
            Value::Tuple(_),
            Value::Integer(_) | Value::Float(_) | Value::Text(_) | Value::Blob(_),
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
/// Compare an integer with a float without losing the integer's precision.
///
/// Casting an `i64` to an `f64` is safe only up to 2^53, past which the float
/// can no longer hold every whole number. Inside that window the cast is used,
/// since a float that survives it is the same float. Outside it, the float is
/// first checked against the ends of an `i64`, and if it lies inside, its whole
/// part is compared with the integer and its fraction settles the tie.
// I could ignore this and cast directly to i64, since no one is likely to use
// a number that large. But why not learn something new?
// [`Source`](https://stackoverflow.com/questions/58734034/how-to-properly-compare-an-integer-and-a-floating-point-value)
pub(crate) fn compare_num(i: i64, f: f64) -> Ordering {
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
            // The integer matches the truncated float whole number part.
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

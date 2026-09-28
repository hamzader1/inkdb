use crate::SqliteResult;
use crate::errors::SqliteError;

use super::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Numeric {
    Int(i64),
    Real(f64),
}

pub trait TryAdd {
    fn try_add(&self, rhs: &Value<'_>) -> SqliteResult<Value<'static>>;
}

pub trait TrySub {
    fn try_sub(&self, rhs: &Value<'_>) -> SqliteResult<Value<'static>>;
}

pub trait TryMul {
    fn try_mul(&self, rhs: &Value<'_>) -> SqliteResult<Value<'static>>;
}

pub trait TryDiv {
    fn try_div(&self, rhs: &Value<'_>) -> SqliteResult<Value<'static>>;
}

fn numeric_prefix_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+') | Some(b'-')) {
        i = 1;
    }
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let mut has_digits = i > digits_start;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        let frac_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        has_digits = has_digits || i > frac_start;
    }
    if !has_digits {
        return 0;
    }
    if matches!(bytes.get(i), Some(b'e') | Some(b'E')) {
        let mut j = i + 1;
        if matches!(bytes.get(j), Some(b'+') | Some(b'-')) {
            j += 1;
        }
        let exp_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    i
}

fn numeric_from_text(text: &str) -> Numeric {
    let trimmed = text.trim_start();
    let len = numeric_prefix_len(trimmed);
    if len == 0 {
        return Numeric::Int(0);
    }
    let literal = &trimmed[..len];
    let integral = literal
        .bytes()
        .all(|b| b.is_ascii_digit() || b == b'-' || b == b'+');
    if integral && let Ok(int) = literal.parse::<i64>() {
        return Numeric::Int(int);
    }
    let mut real = literal.to_string();
    if real.ends_with('.') {
        real.push('0');
    } else if let Some(dot) = real.find(".e").or_else(|| real.find(".E")) {
        real.insert(dot + 1, '0');
    }
    match real.parse::<f64>() {
        Ok(real) => Numeric::Real(real),
        Err(_) => Numeric::Int(0),
    }
}

fn to_numeric(value: &Value<'_>) -> SqliteResult<Option<Numeric>> {
    Ok(match value {
        Value::Null => None,
        Value::Integer(n) => Some(Numeric::Int(*n)),
        Value::Float(f) => Some(Numeric::Real(*f)),
        Value::Text(text) => Some(numeric_from_text(text)),
        Value::Blob(bytes) => Some(match std::str::from_utf8(bytes) {
            Ok(text) => numeric_from_text(text),
            Err(_) => Numeric::Int(0),
        }),
        Value::Tuple(_) => {
            return Err(SqliteError::type_conversion("NUMERIC", value.type_name()));
        }
    })
}

fn operands(lhs: &Value<'_>, rhs: &Value<'_>) -> SqliteResult<Option<(Numeric, Numeric)>> {
    let (Some(lhs), Some(rhs)) = (to_numeric(lhs)?, to_numeric(rhs)?) else {
        return Ok(None);
    };
    Ok(Some((lhs, rhs)))
}

fn int_op(
    a: i64,
    b: i64,
    checked: fn(i64, i64) -> Option<i64>,
    real: fn(f64, f64) -> f64,
) -> Value<'static> {
    match checked(a, b) {
        Some(value) => Value::Integer(value),
        None => Value::Float(real(a as f64, b as f64)),
    }
}

impl TryAdd for Value<'_> {
    fn try_add(&self, rhs: &Value<'_>) -> SqliteResult<Value<'static>> {
        Ok(match operands(self, rhs)? {
            None => Value::Null,
            Some((Numeric::Int(a), Numeric::Int(b))) => {
                int_op(a, b, i64::checked_add, |a, b| a + b)
            }
            Some((Numeric::Int(a), Numeric::Real(b))) => Value::Float(a as f64 + b),
            Some((Numeric::Real(a), Numeric::Int(b))) => Value::Float(a + b as f64),
            Some((Numeric::Real(a), Numeric::Real(b))) => Value::Float(a + b),
        })
    }
}

impl TrySub for Value<'_> {
    fn try_sub(&self, rhs: &Value<'_>) -> SqliteResult<Value<'static>> {
        Ok(match operands(self, rhs)? {
            None => Value::Null,
            Some((Numeric::Int(a), Numeric::Int(b))) => {
                int_op(a, b, i64::checked_sub, |a, b| a - b)
            }
            Some((Numeric::Int(a), Numeric::Real(b))) => Value::Float(a as f64 - b),
            Some((Numeric::Real(a), Numeric::Int(b))) => Value::Float(a - b as f64),
            Some((Numeric::Real(a), Numeric::Real(b))) => Value::Float(a - b),
        })
    }
}

impl TryMul for Value<'_> {
    fn try_mul(&self, rhs: &Value<'_>) -> SqliteResult<Value<'static>> {
        Ok(match operands(self, rhs)? {
            None => Value::Null,
            Some((Numeric::Int(a), Numeric::Int(b))) => {
                int_op(a, b, i64::checked_mul, |a, b| a * b)
            }
            Some((Numeric::Int(a), Numeric::Real(b))) => Value::Float(a as f64 * b),
            Some((Numeric::Real(a), Numeric::Int(b))) => Value::Float(a * b as f64),
            Some((Numeric::Real(a), Numeric::Real(b))) => Value::Float(a * b),
        })
    }
}

impl TryDiv for Value<'_> {
    fn try_div(&self, rhs: &Value<'_>) -> SqliteResult<Value<'static>> {
        Ok(match operands(self, rhs)? {
            None => Value::Null,
            Some((Numeric::Int(a), Numeric::Int(b))) => {
                if b == 0 {
                    Value::Null
                } else {
                    int_op(a, b, i64::checked_div, |a, b| a / b)
                }
            }
            Some((Numeric::Int(a), Numeric::Real(b))) => {
                if b == 0.0 {
                    Value::Null
                } else {
                    Value::Float(a as f64 / b)
                }
            }
            Some((Numeric::Real(a), Numeric::Int(b))) => {
                if b == 0 {
                    Value::Null
                } else {
                    Value::Float(a / b as f64)
                }
            }
            Some((Numeric::Real(a), Numeric::Real(b))) => {
                if b == 0.0 {
                    Value::Null
                } else {
                    Value::Float(a / b)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    fn int(n: i64) -> Value<'static> {
        Value::Integer(n)
    }

    fn real(f: f64) -> Value<'static> {
        Value::Float(f)
    }

    fn text(s: &str) -> Value<'static> {
        Value::Text(Cow::Owned(s.to_string()))
    }

    fn blob(bytes: &[u8]) -> Value<'static> {
        Value::Blob(Cow::Owned(bytes.to_vec()))
    }

    fn value(result: SqliteResult<Value<'static>>) -> Value<'static> {
        result.expect("these operands must not fail")
    }

    #[test]
    fn integer_arithmetic_stays_integer() {
        assert_eq!(value(int(7).try_add(&int(8))), int(15));
        assert_eq!(value(int(7).try_sub(&int(8))), int(-1));
        assert_eq!(value(int(7).try_mul(&int(8))), int(56));
        assert_eq!(value(int(7).try_div(&int(2))), int(3));
        assert_eq!(value(int(-7).try_div(&int(2))), int(-3));
    }

    #[test]
    fn integer_overflow_promotes_to_real() {
        assert_eq!(
            value(int(i64::MAX).try_add(&int(1))),
            real(9_223_372_036_854_775_808.0)
        );
        assert_eq!(
            value(int(i64::MIN).try_sub(&int(1))),
            real(-9_223_372_036_854_775_808.0)
        );
        assert_eq!(
            value(int(i64::MAX).try_mul(&int(2))),
            real(18_446_744_073_709_551_616.0)
        );
        assert_eq!(
            value(int(i64::MIN).try_div(&int(-1))),
            real(9_223_372_036_854_775_808.0)
        );
    }

    #[test]
    fn division_by_zero_is_null() {
        assert_eq!(value(int(7).try_div(&int(0))), Value::Null);
        assert_eq!(value(int(7).try_div(&real(0.0))), Value::Null);
        assert_eq!(value(real(7.0).try_div(&int(0))), Value::Null);
        assert_eq!(value(real(7.0).try_div(&real(0.0))), Value::Null);
    }

    #[test]
    fn mixed_operands_use_real() {
        assert_eq!(value(int(1).try_add(&real(0.5))), real(1.5));
        assert_eq!(value(real(0.5).try_add(&int(1))), real(1.5));
        assert_eq!(value(int(7).try_div(&real(2.0))), real(3.5));
    }

    #[test]
    fn null_propagates_through_every_operator() {
        let null = Value::Null;
        assert_eq!(value(int(1).try_add(&null)), Value::Null);
        assert_eq!(value(null.try_add(&int(1))), Value::Null);
        assert_eq!(value(int(1).try_sub(&null)), Value::Null);
        assert_eq!(value(int(1).try_mul(&null)), Value::Null);
        assert_eq!(value(int(1).try_div(&null)), Value::Null);
    }

    #[test]
    fn text_is_coerced_the_way_coerces_it() {
        assert_eq!(value(text("12").try_add(&int(1))), int(13));
        assert_eq!(value(text(" 12 ").try_add(&int(1))), int(13));
        assert_eq!(value(text("+7").try_add(&int(0))), int(7));
        assert_eq!(value(text("12.5").try_add(&int(1))), real(13.5));
        assert_eq!(value(text("12abc").try_add(&int(1))), int(13));
        assert_eq!(value(text("1_000").try_add(&int(0))), int(1));
        assert_eq!(value(text("abc").try_add(&int(1))), int(1));
        assert_eq!(value(text("+").try_add(&int(0))), int(0));
        assert_eq!(value(text("-").try_add(&int(0))), int(0));
        assert_eq!(value(text(".").try_add(&int(0))), int(0));
        assert_eq!(value(text("").try_add(&int(0))), int(0));
        assert_eq!(value(text("1e").try_add(&int(0))), int(1));
        assert_eq!(value(text(".5").try_add(&int(0))), real(0.5));
        assert_eq!(value(text("-.5").try_add(&int(0))), real(-0.5));
        assert_eq!(value(text("12.").try_add(&int(1))), real(13.0));
        assert_eq!(value(text("12.abc").try_add(&int(0))), real(12.0));
        assert_eq!(value(text("12.e3").try_add(&int(0))), real(12_000.0));
        assert_eq!(value(text("  .5  ").try_add(&int(0))), real(0.5));
        assert_eq!(value(text("1e3").try_add(&int(0))), real(1_000.0));
        assert_eq!(value(text("0x10").try_add(&int(0))), int(0));
        assert_eq!(value(text("-3").try_mul(&int(2))), int(-6));
        assert_eq!(value(text("12").try_add(&text("30"))), int(42));
        assert_eq!(
            value(text("9223372036854775808").try_add(&int(0))),
            real(9_223_372_036_854_775_808.0)
        );
    }

    #[test]
    fn blobs_are_coerced_like_text() {
        assert_eq!(value(blob(b"12").try_add(&int(1))), int(13));
        assert_eq!(value(blob(b"12abc").try_add(&int(1))), int(13));
        assert_eq!(value(blob(b"12.").try_add(&int(0))), real(12.0));
        assert_eq!(value(blob(&[0xff, 0xfe]).try_add(&int(1))), int(1));
        assert_eq!(value(blob(b"").try_add(&int(0))), int(0));
    }

    #[derive(Clone, Copy)]
    enum Op {
        Add,
        Sub,
        Mul,
        Div,
    }

    impl Op {
        fn apply(self, lhs: &Value<'_>, rhs: &Value<'_>) -> SqliteResult<Value<'static>> {
            match self {
                Op::Add => lhs.try_add(rhs),
                Op::Sub => lhs.try_sub(rhs),
                Op::Mul => lhs.try_mul(rhs),
                Op::Div => lhs.try_div(rhs),
            }
        }

        fn sql(self) -> &'static str {
            match self {
                Op::Add => "+",
                Op::Sub => "-",
                Op::Mul => "*",
                Op::Div => "/",
            }
        }
    }

    #[test]
    fn agrees_with_every_case() {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        let cases: &[(&str, Value<'static>, Op, &str, Value<'static>)] = &[
            ("7", int(7), Op::Add, "8", int(8)),
            ("7", int(7), Op::Sub, "8", int(8)),
            ("7", int(7), Op::Mul, "8", int(8)),
            ("7", int(7), Op::Div, "2", int(2)),
            ("-7", int(-7), Op::Div, "2", int(2)),
            ("7", int(7), Op::Div, "0", int(0)),
            ("7.0", real(7.0), Op::Div, "0", int(0)),
            ("7", int(7), Op::Div, "0.0", real(0.0)),
            ("7.0", real(7.0), Op::Div, "0.0", real(0.0)),
            ("1", int(1), Op::Add, "0.5", real(0.5)),
            ("0.5", real(0.5), Op::Add, "1", int(1)),
            ("7", int(7), Op::Div, "2.0", real(2.0)),
            ("9223372036854775807", int(i64::MAX), Op::Add, "1", int(1)),
            ("9223372036854775807", int(i64::MAX), Op::Mul, "2", int(2)),
            (
                "-9223372036854775807 - 1",
                int(i64::MIN),
                Op::Div,
                "-1",
                int(-1),
            ),
            ("NULL", Value::Null, Op::Add, "1", int(1)),
            ("1", int(1), Op::Add, "NULL", Value::Null),
            ("'12'", text("12"), Op::Add, "1", int(1)),
            ("' 12 '", text(" 12 "), Op::Add, "1", int(1)),
            ("'12.5'", text("12.5"), Op::Add, "1", int(1)),
            ("'12abc'", text("12abc"), Op::Add, "1", int(1)),
            ("'abc'", text("abc"), Op::Add, "1", int(1)),
            ("'12.'", text("12."), Op::Add, "1", int(1)),
            ("'12.e3'", text("12.e3"), Op::Add, "0", int(0)),
            ("'.5'", text(".5"), Op::Add, "0", int(0)),
            ("''", text(""), Op::Add, "0", int(0)),
            ("'12'", text("12"), Op::Add, "'30'", text("30")),
            ("x'3132'", blob(b"12"), Op::Add, "1", int(1)),
            ("x'3132616263'", blob(b"12abc"), Op::Add, "1", int(1)),
            ("x'31322e'", blob(b"12."), Op::Add, "0", int(0)),
            ("x'fffe'", blob(&[0xff, 0xfe]), Op::Add, "1", int(1)),
        ];
        for (lhs_sql, lhs, op, rhs_sql, rhs) in cases {
            let expr = format!("({lhs_sql}) {} ({rhs_sql})", op.sql());
            let expected: String = conn
                .query_row(&format!("SELECT typeof({expr})"), [], |row| row.get(0))
                .unwrap_or_else(|e| panic!("{expr}: {e}"));
            let got = op.apply(lhs, rhs).expect("arithmetic must not fail");
            assert_eq!(got.type_name().to_lowercase(), expected, "type of {expr}");
            if !matches!(got, Value::Null) {
                let equal: bool = conn
                    .query_row(
                        &format!("SELECT ({expr}) IS (CAST('{got}' AS NUMERIC))"),
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or_else(|e| panic!("{expr}: {e}"));
                assert!(equal, "{expr}: we computed {got}, sqlite disagrees");
            }
        }
    }

    #[test]
    fn every_operand_pair_returns_instead_of_panicking() {
        let values = [
            Value::Null,
            int(0),
            int(1),
            int(-1),
            int(i64::MIN),
            int(i64::MAX),
            real(0.0),
            real(1.5),
            real(f64::MAX),
            text("12"),
            text("abc"),
            blob(b"12"),
            blob(&[0xff, 0xfe]),
            Value::Tuple(vec![int(1)]),
        ];
        for lhs in &values {
            for rhs in &values {
                let _ = lhs.try_add(rhs);
                let _ = lhs.try_sub(rhs);
                let _ = lhs.try_mul(rhs);
                let _ = lhs.try_div(rhs);
            }
        }
    }

    #[test]
    fn tuples_are_an_error_not_a_panic() {
        let tuple = Value::Tuple(vec![int(1), int(2)]);
        assert!(int(1).try_add(&tuple).is_err());
        assert!(tuple.try_mul(&int(2)).is_err());
        assert!(tuple.try_div(&int(2)).is_err());
    }
}

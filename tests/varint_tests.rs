//! Tests for `inkdb::format::varint` (re-exported at crate root as
//! `encode_varint` / `decode_varint`).
//!
//! Round-trip and boundary coverage already exists inside varint.rs itself;
//! these tests focus on `decode_varint`'s behavior on malformed/adversarial
//! input, since that's the function that will see untrusted bytes off disk.

use inkdb::{decode_varint, encode_varint};

#[test]
fn decode_empty_slice_returns_none() {
    assert_eq!(decode_varint(&[]), None);
}

#[test]
fn decode_all_continuation_bits_set_and_too_short_returns_none() {
    // 5 bytes, all with high bit set, no terminator -> incomplete varint.
    let bytes = [0x80, 0x80, 0x80, 0x80, 0x80];
    assert_eq!(decode_varint(&bytes), None);
}

#[test]
fn decode_single_zero_byte() {
    assert_eq!(decode_varint(&[0x00]), Some((0, 1)));
}

#[test]
fn decode_stops_after_terminator_ignores_trailing_bytes() {
    // terminator on first byte; trailing garbage should be ignored, and
    // bytes_read should reflect only what was consumed.
    let bytes = [0x05, 0xFF, 0xFF, 0xFF];
    assert_eq!(decode_varint(&bytes), Some((5, 1)));
}

#[test]
fn decode_two_byte_value() {
    // 0x81 0x00 -> (1 << 7) | 0 = 128
    let bytes = [0x81, 0x00];
    assert_eq!(decode_varint(&bytes), Some((128, 2)));
}

#[test]
fn decode_ninth_byte_uses_full_8_bits_no_continuation_check() {
    // 8 bytes with continuation bit set (7 bits each) + a 9th byte that
    // should be taken as a full 8-bit value regardless of its high bit.
    let mut bytes = [0x81u8; 9];
    bytes[8] = 0xFF; // full 8 bits, would look like "continue" if the
    // continuation-bit rule were (wrongly) applied here.
    let result = decode_varint(&bytes);
    assert!(result.is_some());
    let (_, used) = result.unwrap();
    assert_eq!(used, 9, "must always terminate by the 9th byte");
}

#[test]
fn decode_exactly_9_bytes_present_terminates() {
    let bytes = [0xFFu8; 9];
    let (_, used) = decode_varint(&bytes).unwrap();
    assert_eq!(used, 9);
}

#[test]
fn decode_max_u64_round_trip() {
    let mut buf = [0u8; 9];
    let len = encode_varint(&mut buf, u64::MAX);
    assert_eq!(len, 9);
    let (val, used) = decode_varint(&buf).unwrap();
    assert_eq!(val, u64::MAX);
    assert_eq!(used, 9);
}

#[test]
fn decode_zero_round_trip() {
    let mut buf = [0u8; 9];
    let len = encode_varint(&mut buf, 0);
    assert_eq!(len, 1);
    assert_eq!(buf[0], 0);
    let (val, used) = decode_varint(&buf[..len]).unwrap();
    assert_eq!(val, 0);
    assert_eq!(used, 1);
}

#[test]
fn encode_does_not_touch_bytes_past_returned_length_for_short_values() {
    let mut buf = [0xAAu8; 9];
    let len = encode_varint(&mut buf, 1);
    assert_eq!(len, 1);
    assert_eq!(buf[0], 1);
    // Bytes past len are untouched (still the sentinel) - not a correctness
    // requirement of the format, but documents current behavior so a
    // regression here is caught if callers ever rely on buf being cleared.
    assert_eq!(buf[1], 0xAA);
}

#[test]
fn decode_value_that_needs_all_7_continuation_bytes_plus_terminator() {
    // Round-trip a value requiring exactly 8 varint bytes (56 data bits).
    let value: u64 = 562_949_953_421_312; // per varint.rs test_eight_byte_boundaries
    let mut buf = [0u8; 9];
    let len = encode_varint(&mut buf, value);
    assert_eq!(len, 8);
    let (decoded, used) = decode_varint(&buf[..len]).unwrap();
    assert_eq!(decoded, value);
    assert_eq!(used, 8);
}

#[test]
fn decode_truncated_multibyte_varint_is_none() {
    // Encode a value that needs 3 bytes, then only give decode_varint 2.
    let mut buf = [0u8; 9];
    let len = encode_varint(&mut buf, 100_000);
    assert!(len >= 3);
    assert_eq!(decode_varint(&buf[..2]), None);
}

#[test]
fn round_trip_common_small_values_used_as_serial_types() {
    // Values commonly seen as SQLite record serial types / varint lengths.
    for v in [0u64, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13] {
        let mut buf = [0u8; 9];
        let len = encode_varint(&mut buf, v);
        let (decoded, used) = decode_varint(&buf[..len]).unwrap();
        assert_eq!(decoded, v);
        assert_eq!(used, len);
    }
}

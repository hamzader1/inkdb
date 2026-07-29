use inkdb::{decode_varint, encode_varint};

fn round_trip(value: u64) {
    let mut bytes = [0; 9];
    let length = encode_varint(&mut bytes, value);
    assert_eq!(decode_varint(&bytes[..length]), Some((value, length)));
}

#[test]
fn sqlite_varint_boundaries_round_trip() {
    for value in [
        0,
        1,
        126,
        127,
        128,
        129,
        16_383,
        16_384,
        2_097_151,
        2_097_152,
        268_435_455,
        268_435_456,
        34_359_738_367,
        34_359_738_368,
        4_398_046_511_103,
        4_398_046_511_104,
        562_949_953_421_311,
        562_949_953_421_312,
        72_057_594_037_927_935,
        72_057_594_037_927_936,
        u64::MAX,
    ] {
        round_trip(value);
    }
}

#[test]
fn first_hundred_thousand_values_round_trip() {
    for value in 0..100_000 {
        round_trip(value);
    }
}

#[test]
fn malformed_and_sliced_varints_never_panic() {
    assert_eq!(decode_varint(&[]), None);
    assert_eq!(decode_varint(&[0x80]), None);
    assert_eq!(decode_varint(&[0x80; 8]), None);
    let encoded = [0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x01];
    for length in 0..encoded.len() {
        assert_eq!(decode_varint(&encoded[..length]), None);
    }
    for length in 0..16 {
        for fill in [0, 0x7f, 0x80, 0xff] {
            let _ = decode_varint(&vec![fill; length]);
        }
    }
}

fn check(value: u64, expected_len: usize) {
    let mut buf = [0u8; 9];
    let len = encode_varint(&mut buf, value);

    assert_eq!(len, expected_len);
}

#[test]
fn test_one_byte_boundaries() {
    check(0, 1);
    check(1, 1);
    check(126, 1);
    check(127, 1);
}

#[test]
fn test_two_byte_boundaries() {
    check(128, 2);
    check(129, 2);
    check(16382, 2);
    check(16383, 2);
}

#[test]
fn test_three_byte_boundaries() {
    check(16384, 3);
    check(16385, 3);
    check(2_097_150, 3);
    check(2_097_151, 3);
}

#[test]
fn test_four_byte_boundaries() {
    check(2_097_152, 4);
    check(268_435_454, 4);
    check(268_435_455, 4);
}

#[test]
fn test_five_byte_boundaries() {
    check(268_435_456, 5);
    check(34_359_738_366, 5);
    check(34_359_738_367, 5);
}

#[test]
fn test_six_byte_boundaries() {
    check(34_359_738_368, 6);
    check(4_398_046_511_102, 6);
    check(4_398_046_511_103, 6);
}

#[test]
fn test_seven_byte_boundaries() {
    check(4_398_046_511_104, 7);
    check(562_949_953_421_310, 7);
    check(562_949_953_421_311, 7);
}

#[test]
fn test_eight_byte_boundaries() {
    check(562_949_953_421_312, 8);
    check(72_057_594_037_927_934, 8);
    check(72_057_594_037_927_935, 8);
}

#[test]
fn test_nine_byte_boundaries() {
    check(72_057_594_037_927_936, 9);
    check(u64::MAX, 9);
}
#[test]
fn round_trip_2() {
    let values = [
        0,
        1,
        127,
        128,
        16_383,
        16_384,
        2_097_151,
        2_097_152,
        268_435_455,
        268_435_456,
        34_359_738_367,
        34_359_738_368,
        4_398_046_511_103,
        4_398_046_511_104,
        562_949_953_421_311,
        562_949_953_421_312,
        72_057_594_037_927_935,
        72_057_594_037_927_936,
        u64::MAX,
    ];

    for value in values {
        let mut buf = [0u8; 9];
        let len = encode_varint(&mut buf, value);

        let (decoded, used) = decode_varint(&buf[..len]).unwrap();

        assert_eq!(decoded, value);
        assert_eq!(used, len);
    }
}
#[test]
fn round_trip_first_100_000() {
    for value in 0u64..100_000 {
        let mut buf = [0u8; 9];

        let len = encode_varint(&mut buf, value);
        let (decoded, used) = decode_varint(&buf[..len]).unwrap();

        assert_eq!(decoded, value);
        assert_eq!(used, len);
    }
}
#[test]
fn round_trip_powers_of_two() {
    for bit in 0..64 {
        let value = 1u64 << bit;

        let mut buf = [0u8; 9];

        let len = encode_varint(&mut buf, value);
        let (decoded, used) = decode_varint(&buf[..len]).unwrap();

        assert_eq!(decoded, value);
        assert_eq!(used, len);
    }
}
#[test]
fn round_trip_powers_of_two_plus_one() {
    for bit in 0..63 {
        let value = (1u64 << bit) + 1;

        let mut buf = [0u8; 9];

        let len = encode_varint(&mut buf, value);
        let (decoded, used) = decode_varint(&buf[..len]).unwrap();

        assert_eq!(decoded, value);
        assert_eq!(used, len);
    }
}
#[test]
fn round_trip_powers_of_two_minus_one() {
    for bit in 1..64 {
        let value = (1u64 << bit) - 1;

        let mut buf = [0u8; 9];

        let len = encode_varint(&mut buf, value);
        let (decoded, used) = decode_varint(&buf[..len]).unwrap();

        assert_eq!(decoded, value);
        assert_eq!(used, len);
    }
}

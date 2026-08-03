/*
** The variable-length integer encoding is as follows:
**
** KEY:
**         A = 0xxxxxxx    7 bits of data and one flag bit
**         B = 1xxxxxxx    7 bits of data and one flag bit
**         C = xxxxxxxx    8 bits of data
**
**  7 bits - A
** 14 bits - BA
** 21 bits - BBA
** 28 bits - BBBA
** 35 bits - BBBBA
** 42 bits - BBBBBA
** 49 bits - BBBBBBA
** 56 bits - BBBBBBBA
** 64 bits - BBBBBBBBC
*/

/*
** Write a 64-bit variable-length integer to memory starting at p[0].
** The length of data write will be between 1 and 9 bytes.  The number
** of bytes written is returned.
**
** A variable-length integer consists of the lower 7 bits of each byte
** for all bytes that have the 8th bit set and one byte with the 8th
** bit clear.  Except, if we get to the 9th byte, it stores the full
** 8 bits and is the last byte.
*/

use std::io::{Read, Seek};

use crate::errors::SqliteError;

/*
** References:
*   * https://github.com/sqlite/sqlite/blob/master/src/util.c#L1586
*   * https://github.com/sqlite/sqlite/blob/master/src/util.c#L1610
**
*/
pub fn encode_varint(buff: &mut [u8; 9], mut value: u64) -> usize {
    let mut temp_buffer = [0u8; 10];
    // in case v=0
    if value == 0 {
        buff[0] = 0;
        return 1;
    }
    // in case v len = 9
    if value & 0xff00000000000000 != 0 {
        let mut byte = value as u8;
        buff[8] = byte;
        value >>= 8;

        for i in (0..8).rev() {
            byte = value as u8;
            buff[i] = (byte & 0x7f) | 0x80;
            value >>= 7;
        }
        return 9;
    }
    let mut n: usize = 0;
    // in case v len < 9
    while value != 0 {
        let byte = value as u8;
        temp_buffer[n] = (byte & 0x7f) | 0x80;
        n += 1;
        value >>= 7;
    }
    temp_buffer[0] &= 0x7f;

    // This branch is equivalent to matching n <= 8, because if n == 9
    // the execution flow will never reach this point. In practice, the
    // general condition is n <= 9, but due to earlier checks, the case
    // where n == 9 is already excluded before arriving here.
    assert!(n <= 9);
    // let mut j = n - 1;
    let mut i = 0;
    for j in (0..n).rev() {
        buff[i] = temp_buffer[j];
        i += 1;
    }
    return n;
}

pub fn decode_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;

    for (i, &byte) in bytes.iter().enumerate() {
        if i == 8 {
            value <<= 8;
            value |= byte as u64;
            return Some((value, i + 1));
        } else {
            value = (value << 7) | ((byte & 0x7f) as u64);
        }
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }

    None
}
pub fn remaining_varint_bytes<R: Read + Seek>(
    r: &mut R,
    usable_size: usize,
) -> Result<usize, SqliteError> {
    let cursor_pos = r.stream_position()? as usize;

    let remaining = usable_size
        .checked_sub(cursor_pos)
        .ok_or(SqliteError::InvalidVarint)?;

    Ok(remaining.min(9))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn round_trip() {
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
}

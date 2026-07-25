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

/*
** References:
*   1) https://github.com/sqlite/sqlite/blob/master/src/util.c#L1586
*   2) https://github.com/sqlite/sqlite/blob/master/src/util.c#L1610
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

    // same as we match using n <= 8, since if n == 9 we wont get here
    // but in general n <= 9
    assert!(n <= 9);
    // let mut j = n - 1;
    let mut i = 0;
    for j in (0..n).rev() {
        buff[i] = temp_buffer[j];
        i += 1;
    }
    return n;
}

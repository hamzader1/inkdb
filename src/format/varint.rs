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




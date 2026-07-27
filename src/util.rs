use crate::SqliteDatabaseError;
pub fn assert_one(condition: bool, err: SqliteDatabaseError) -> Result<(), SqliteDatabaseError> {
    if !condition {
        return Err(err);
    }
    Ok(())
}

pub const fn compute_local_payload_size(usable_size: usize, payload_len: usize) -> usize {
    let u = usable_size;
    let p = payload_len;
    let x = u - 35;
    if p <= x {
        return p;
    } else {
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + (p - m) % (u - 4);
        if k <= x {
            return k;
        } else {
            return m;
        }
    }
}

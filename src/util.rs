use crate::SqliteDatabaseError;
pub fn assert_one(condition: bool, err: SqliteDatabaseError) -> Result<(), SqliteDatabaseError> {
    if !condition {
        return Err(err);
    }
    Ok(())
}

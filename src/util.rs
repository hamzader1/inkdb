use crate::SqliteDatabaseError;
pub fn sqlite_assert_one(condition: bool, err: SqliteDatabaseError) -> Result<(), SqliteDatabaseError> {
    if !condition {
        return Err(err);
    }
    Ok(())
}

pub fn sqlite_assert_with_corrupt_err(condition: bool, err: &str) -> Result<(), SqliteDatabaseError> {
    if !condition {
        return Err(SqliteDatabaseError::Corrupt(err.into()));
    }
    Ok(())
}

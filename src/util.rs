use crate::SqliteError;
pub fn sqlite_assert_one(condition: bool, err: SqliteError) -> Result<(), SqliteError> {
    if !condition {
        return Err(err);
    }
    Ok(())
}

pub fn sqlite_assert_with_corrupt_err(condition: bool, err: &str) -> Result<(), SqliteError> {
    if !condition {
        return Err(SqliteError::Corrupt(err.into()));
    }
    Ok(())
}

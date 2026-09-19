use crate::SqliteError;
use crate::pager::pager::PageNo;
pub fn sqlite_assert_one(condition: bool, err: SqliteError) -> Result<(), SqliteError> {
    if !condition {
        return Err(err);
    }
    Ok(())
}

pub fn sqlite_assert_with_corrupt_err<Fn>(condition: bool, err: Fn) -> Result<(), SqliteError>
where
    Fn: FnOnce() -> String,
{
    if !condition {
        return Err(SqliteError::Corrupt(err()));
    }
    Ok(())
}
pub fn sqlite_assert_with_runtime_err<Fn>(condition: bool, err: Fn) -> Result<(), SqliteError>
where
    Fn: FnOnce() -> String,
{
    if !condition {
        return Err(SqliteError::Runtime(err()));
    }
    Ok(())
}

pub fn sqlite_assert_with_internal_err<Fn>(condition: bool, err: Fn) -> Result<(), SqliteError>
where
    Fn: FnOnce() -> String,
{
    if !condition {
        return Err(SqliteError::Internal(err()));
    }
    Ok(())
}

pub fn validate_page(page_no: PageNo, max_pages: usize) -> Result<(), SqliteError>
where
{
    if page_no == 0 || page_no as usize > max_pages {
        return Err(SqliteError::InvalidPageNumber(page_no));
    }

    Ok(())
}
pub fn validate_page_non_one(page_no: PageNo, max_pages: usize) -> Result<(), SqliteError>
where
{
    if page_no == 0 || page_no == 1 || page_no as usize > max_pages {
        return Err(SqliteError::InvalidPageNumber(page_no));
    }

    Ok(())
}

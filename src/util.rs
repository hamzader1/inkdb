use crate::SqliteError;
use crate::pager::pager::PageNo;
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
pub fn sqlite_assert_with_runtime_err(condition: bool, err: &str) -> Result<(), SqliteError> {
    if !condition {
        return Err(SqliteError::Runtime(err.into()));
    }
    Ok(())
}

pub fn validate_page<E>(
    page_no: PageNo,
    max_pages: usize,
    exception: Option<E>,
) -> Result<(), SqliteError>
where
    E: Fn(PageNo) -> bool,
{
    if let Some(exc) = exception
        && exc(page_no)
    {
        return Err(SqliteError::Internal(format!(
            "page guard exception rejected page {page_no}"
        )));
    }
    if page_no == 0 || page_no as usize > max_pages {
        return Err(SqliteError::InvalidPageNumber(page_no));
    }

    Ok(())
}

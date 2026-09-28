use crate::InkError;
use crate::errors::CorruptError;
use crate::pager::pager::PageNo;
pub fn assert_one(condition: bool, err: InkError) -> Result<(), InkError> {
    if !condition {
        return Err(err);
    }
    Ok(())
}

pub fn assert_with_corrupt_err<Fn>(condition: bool, err: Fn) -> Result<(), InkError>
where
    Fn: FnOnce() -> String,
{
    if !condition {
        return Err(CorruptError::Assertion(err()).into());
    }
    Ok(())
}
pub fn assert_with_runtime_err<Fn>(condition: bool, err: Fn) -> Result<(), InkError>
where
    Fn: FnOnce() -> String,
{
    if !condition {
        return Err(InkError::runtime(err()));
    }
    Ok(())
}

pub fn assert_with_internal_err<Fn>(condition: bool, err: Fn) -> Result<(), InkError>
where
    Fn: FnOnce() -> String,
{
    if !condition {
        return Err(InkError::InternalFmt(err()));
    }
    Ok(())
}

pub fn validate_page(page_no: PageNo, max_pages: usize) -> Result<(), InkError>
where
{
    if page_no == 0 || page_no as usize > max_pages {
        return Err(InkError::InvalidPageNumber(page_no));
    }

    Ok(())
}
pub fn validate_page_non_one(page_no: PageNo, max_pages: usize) -> Result<(), InkError>
where
{
    if page_no == 0 || page_no == 1 || page_no as usize > max_pages {
        return Err(InkError::InvalidPageNumber(page_no));
    }

    Ok(())
}

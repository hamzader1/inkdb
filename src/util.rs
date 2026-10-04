use crate::InkError;
use crate::errors::CorruptError;
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

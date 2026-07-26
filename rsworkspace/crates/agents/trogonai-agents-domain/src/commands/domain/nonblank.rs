#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NonBlankViolationError {
    #[error("must not be empty")]
    Empty,
    #[error("must not have leading or trailing whitespace")]
    SurroundingWhitespace,
}

pub(crate) fn validate_nonblank(raw: &str) -> Result<(), NonBlankViolationError> {
    if raw.is_empty() {
        return Err(NonBlankViolationError::Empty);
    }
    if raw.trim() != raw {
        return Err(NonBlankViolationError::SurroundingWhitespace);
    }
    Ok(())
}

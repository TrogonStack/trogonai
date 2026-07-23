#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NonBlankViolation {
    #[error("must not be empty")]
    Empty,
    #[error("must not have leading or trailing whitespace")]
    SurroundingWhitespace,
}

pub(crate) fn validate_nonblank(raw: &str) -> Result<(), NonBlankViolation> {
    if raw.is_empty() {
        return Err(NonBlankViolation::Empty);
    }
    if raw.trim() != raw {
        return Err(NonBlankViolation::SurroundingWhitespace);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerValueError {
    #[error("peer value must not be empty")]
    Empty,
    #[error("peer value exceeds {maximum} UTF-8 bytes")]
    TooLong { maximum: usize },
    #[error("peer value contains a disallowed character")]
    InvalidCharacter,
}

pub(super) fn parse_token(value: String) -> Result<String, PeerValueError> {
    if value.is_empty() {
        return Err(PeerValueError::Empty);
    }
    if value.len() > 128 {
        return Err(PeerValueError::TooLong { maximum: 128 });
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(PeerValueError::InvalidCharacter);
    }
    Ok(value)
}

pub(super) fn parse_text(value: String, maximum: usize) -> Result<String, PeerValueError> {
    if value.is_empty() {
        return Err(PeerValueError::Empty);
    }
    if value.len() > maximum {
        return Err(PeerValueError::TooLong { maximum });
    }
    if value.chars().any(char::is_control) {
        return Err(PeerValueError::InvalidCharacter);
    }
    Ok(value)
}

use crate::TokenError;
use crate::constants::MAXIMUM_BYTES;

pub(crate) fn validate_token(value: &str) -> Result<(), TokenError> {
    if value.is_empty() {
        return Err(TokenError::Empty);
    }
    if value.len() > MAXIMUM_BYTES {
        return Err(TokenError::TooLong {
            actual: value.len(),
            maximum: MAXIMUM_BYTES,
        });
    }
    if let Some(index) = value
        .bytes()
        .position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    {
        return Err(TokenError::InvalidCharacter { index });
    }
    Ok(())
}

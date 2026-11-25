use crate::UnixNameError;
use crate::constants::MAX_UNIX_NAME_LEN;

pub(crate) fn validate_unix_name(name: &str) -> Result<(), UnixNameError> {
    if name.is_empty() || name.len() > MAX_UNIX_NAME_LEN {
        return Err(UnixNameError::InvalidLength);
    }
    let mut bytes = name.bytes();
    let starts_well = bytes
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == b'_');
    if !starts_well
        || !bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-'))
    {
        return Err(UnixNameError::InvalidCharacters);
    }
    Ok(())
}

use crate::VercelApiError;
use crate::constants::MAX_ID_LEN;

/// Ids that reach a URL path are limited to a safe alphabet.
pub(crate) fn safe_id(id: &str, what: &str) -> Result<(), VercelApiError> {
    let safe = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if safe {
        Ok(())
    } else {
        Err(VercelApiError::Rejected {
            status: 400,
            message: format!("invalid {what}"),
        })
    }
}

#[cfg(test)]
mod tests;

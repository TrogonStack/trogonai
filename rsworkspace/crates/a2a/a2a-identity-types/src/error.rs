#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    #[error("JWT decode error: {0}")]
    Decode(String),
    #[error("system time error: {0}")]
    SystemTime(#[source] std::time::SystemTimeError),
    #[error("caller_id invalid for NATS subject token")]
    InvalidCallerId,
    #[error("external subject must be non-empty")]
    InvalidExternalSubject,
    #[error("issued-at timestamp out of portable range")]
    IssuedAtOutOfRange,
}

#[cfg(test)]
mod tests;

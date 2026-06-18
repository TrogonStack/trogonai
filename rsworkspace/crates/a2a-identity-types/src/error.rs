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
mod tests {
    use super::*;

    fn system_time_error() -> std::time::SystemTimeError {
        let now = std::time::SystemTime::now();
        now.duration_since(now + std::time::Duration::from_secs(60))
            .unwrap_err()
    }

    #[test]
    fn display_covers_every_variant() {
        assert!(JwtError::Decode("oops".into()).to_string().contains("oops"));
        assert!(
            JwtError::SystemTime(system_time_error())
                .to_string()
                .contains("system time")
        );
        assert!(JwtError::InvalidCallerId.to_string().contains("caller_id"));
        assert!(JwtError::InvalidExternalSubject.to_string().contains("external"));
        assert!(JwtError::IssuedAtOutOfRange.to_string().contains("out of"));
    }

    #[test]
    fn system_time_variant_exposes_source() {
        let err = JwtError::SystemTime(system_time_error());
        assert!(std::error::Error::source(&err).is_some());
        assert!(std::error::Error::source(&JwtError::InvalidCallerId).is_none());
    }
}

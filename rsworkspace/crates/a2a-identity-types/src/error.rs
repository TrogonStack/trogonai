use std::fmt;

#[derive(Debug)]
pub enum JwtError {
    Decode(String),
    SystemTime(std::time::SystemTimeError),
    InvalidCallerId,
    InvalidExternalSubject,
    IssuedAtOutOfRange,
}

impl fmt::Display for JwtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "JWT decode error: {e}"),
            Self::SystemTime(e) => write!(f, "system time error: {e}"),
            Self::InvalidCallerId => f.write_str("caller_id invalid for NATS subject token"),
            Self::InvalidExternalSubject => f.write_str("external subject must be non-empty"),
            Self::IssuedAtOutOfRange => f.write_str("issued-at timestamp out of portable range"),
        }
    }
}

impl std::error::Error for JwtError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SystemTime(e) => Some(e),
            _ => None,
        }
    }
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

use std::sync::Arc;
use std::time::Duration;
use trogon_nats::NatsConfig;

const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PREFIX_LENGTH: usize = 128;

/// Errors returned when [`Config`] values contain empty strings or NATS-unsafe characters.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationError {
    EmptyValue(&'static str),
    InvalidCharacter(&'static str, char),
    TooLong(&'static str, usize),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::EmptyValue(field) => write!(f, "{} must not be empty", field),
            ValidationError::InvalidCharacter(field, ch) => {
                write!(f, "{} contains invalid character: {:?}", field, ch)
            }
            ValidationError::TooLong(field, len) => {
                write!(f, "{} is too long: {} bytes (max 128)", field, len)
            }
        }
    }
}

impl std::error::Error for ValidationError {}

fn has_nats_wildcards_or_whitespace(value: &str) -> Option<char> {
    value
        .chars()
        .find(|ch| *ch == '*' || *ch == '>' || ch.is_whitespace())
}

fn has_consecutive_or_boundary_dots(value: &str) -> bool {
    value.contains("..") || value.starts_with('.') || value.ends_with('.')
}

#[derive(Clone)]
pub struct AcpPrefix(Arc<str>);

impl AcpPrefix {
    pub fn new(s: impl Into<String>) -> Result<Self, ValidationError> {
        let s = s.into();
        if s.is_empty() {
            return Err(ValidationError::EmptyValue("acp_prefix"));
        }
        if let Some(ch) = has_nats_wildcards_or_whitespace(&s) {
            return Err(ValidationError::InvalidCharacter("acp_prefix", ch));
        }
        if has_consecutive_or_boundary_dots(&s) {
            return Err(ValidationError::InvalidCharacter("acp_prefix", '.'));
        }
        if s.len() > MAX_PREFIX_LENGTH {
            return Err(ValidationError::TooLong("acp_prefix", s.len()));
        }
        Ok(Self(s.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone)]
pub struct Config {
    pub(crate) acp_prefix: AcpPrefix,
    pub(crate) nats: NatsConfig,
    pub(crate) operation_timeout: Duration,
}

impl Config {
    pub fn new(acp_prefix: AcpPrefix, nats: NatsConfig) -> Self {
        Self {
            acp_prefix,
            nats,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
        }
    }

    pub fn with_operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout;
        self
    }

    pub fn acp_prefix(&self) -> &str {
        self.acp_prefix.as_str()
    }

    pub fn nats(&self) -> &NatsConfig {
        &self.nats
    }

    pub fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    #[cfg(test)]
    pub(crate) fn for_test(acp_prefix: &str) -> Self {
        let nats = NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        };
        Self::new(AcpPrefix::new(acp_prefix).unwrap(), nats)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Duration;

    use super::*;

    fn default_nats() -> NatsConfig {
        NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        }
    }

    #[test]
    fn config_new_accepts_validated_prefix() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats());
        assert_eq!(config.acp_prefix(), "acp");
    }

    #[test]
    fn acp_prefix_new_valid() {
        let p = AcpPrefix::new("acp").unwrap();
        assert_eq!(p.as_str(), "acp");
        assert_eq!(
            AcpPrefix::new("my.multi.part").unwrap().as_str(),
            "my.multi.part"
        );
    }

    #[test]
    fn acp_prefix_new_invalid_returns_err() {
        assert!(AcpPrefix::new("").is_err());
        assert!(AcpPrefix::new("acp.*").is_err());
        assert!(AcpPrefix::new("acp.>").is_err());
        assert!(AcpPrefix::new("acp prefix").is_err());
        assert!(AcpPrefix::new("acp\t").is_err());
        assert!(AcpPrefix::new("acp\n").is_err());
        assert!(AcpPrefix::new("acp..foo").is_err());
        assert!(AcpPrefix::new(".acp").is_err());
        assert!(AcpPrefix::new("acp.").is_err());
        assert!(AcpPrefix::new("a".repeat(129)).is_err());
    }

    #[test]
    fn acp_prefix_new_validates_direct() {
        assert!(AcpPrefix::new("acp").is_ok());
        assert!(AcpPrefix::new("a").is_ok());
        assert!(AcpPrefix::new("my.multi.part").is_ok());
        assert!(AcpPrefix::new("a".repeat(128)).is_ok());
        assert!(matches!(
            AcpPrefix::new(""),
            Err(ValidationError::EmptyValue(_))
        ));
        assert!(matches!(
            AcpPrefix::new("a".repeat(129)),
            Err(ValidationError::TooLong(_, 129))
        ));
    }

    #[test]
    fn validation_error_display() {
        assert_eq!(
            format!("{}", ValidationError::EmptyValue("acp_prefix")),
            "acp_prefix must not be empty"
        );
        assert_eq!(
            format!("{}", ValidationError::InvalidCharacter("acp_prefix", '*')),
            "acp_prefix contains invalid character: '*'"
        );
        assert_eq!(
            format!("{}", ValidationError::TooLong("acp_prefix", 200)),
            "acp_prefix is too long: 200 bytes (max 128)"
        );
    }

    #[test]
    fn config_with_operation_timeout() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats())
            .with_operation_timeout(Duration::from_secs(60));
        assert_eq!(config.operation_timeout(), Duration::from_secs(60));
    }

    #[test]
    fn config_nats_returns_nats_config() {
        let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats());
        assert_eq!(config.nats().servers.len(), 1);
        assert_eq!(config.nats().servers[0], "localhost:4222");
    }

    #[test]
    fn validation_error_impl_error() {
        let err = ValidationError::EmptyValue("field");
        assert!(err.source().is_none());
    }
}

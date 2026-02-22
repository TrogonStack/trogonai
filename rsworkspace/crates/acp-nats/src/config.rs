use std::sync::Arc;
use std::time::Duration;
use trogon_nats::NatsConfig;

use crate::nats::token;

const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_PROMPT_TIMEOUT: Duration = Duration::from_secs(7200);
const DEFAULT_MAX_CONCURRENT_CLIENT_TASKS: usize = 256;
const DEFAULT_NATS_MAX_PAYLOAD_BYTES: usize = 1_048_576;
const NATS_MAX_SUBJECT_BYTES: usize = 256;
const MAX_PREFIX_LENGTH: usize = 128;
const MAX_SESSION_ID_LENGTH: usize = 128;
const MAX_METHOD_NAME_LENGTH: usize = 128;
// Keep this list synchronized with `acp_nats::nats::ClientMethod` suffixes.
// If a new client subject suffix is added, extend this list and update
// MAX_SESSIONED_SUBJECT_SUFFIX_LEN to avoid silently weakening subject-capacity checks.
const MAX_SESSIONED_SUBJECT_SUFFIX_LEN: usize = ".client.ext.session.prompt_response".len();

/// Runtime configuration for the ACP-NATS bridge.
#[derive(Clone)]
pub struct Config {
    pub(crate) acp_prefix: String,
    pub(crate) nats: NatsConfig,
    pub(crate) operation_timeout: Duration,
    pub(crate) prompt_timeout: Duration,
    pub(crate) max_concurrent_client_tasks: usize,
    pub(crate) max_nats_payload_bytes: usize,
}

/// Errors returned when [`Config`] values contain empty strings or NATS-unsafe characters.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationError {
    EmptyValue(&'static str),
    InvalidCharacter(&'static str, char),
    TooLong(&'static str, usize),
    CombinedSubjectsTooLong(usize),
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
            ValidationError::CombinedSubjectsTooLong(len) => {
                write!(
                    f,
                    "acp_prefix and session_id exceed NATS subject limits at worst-case: {} bytes (max {})",
                    len,
                    NATS_MAX_SUBJECT_BYTES
                )
            }
        }
    }
}

impl std::error::Error for ValidationError {}

#[derive(Clone)]
pub struct AcpPrefix(Arc<str>);

impl AcpPrefix {
    pub fn new(s: impl Into<String>) -> Result<Self, ValidationError> {
        let s = s.into();
        if s.is_empty() {
            return Err(ValidationError::EmptyValue("acp_prefix"));
        }
        if let Some(ch) = token::has_wildcards_or_whitespace(&s) {
            return Err(ValidationError::InvalidCharacter("acp_prefix", ch));
        }
        if token::has_consecutive_or_boundary_dots(&s) {
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

impl std::ops::Deref for AcpPrefix {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub fn validate_prefix(prefix: &str) -> Result<(), ValidationError> {
    if prefix.is_empty() {
        return Err(ValidationError::EmptyValue("acp_prefix"));
    }
    if let Some(ch) = token::has_wildcards_or_whitespace(prefix) {
        return Err(ValidationError::InvalidCharacter("acp_prefix", ch));
    }
    if token::has_consecutive_or_boundary_dots(prefix) {
        return Err(ValidationError::InvalidCharacter("acp_prefix", '.'));
    }
    if prefix.len() > MAX_PREFIX_LENGTH {
        return Err(ValidationError::TooLong("acp_prefix", prefix.len()));
    }
    Ok(())
}

pub fn validate_method_name(method: &str) -> Result<(), ValidationError> {
    if method.is_empty() {
        return Err(ValidationError::EmptyValue("method"));
    }
    if method.len() > MAX_METHOD_NAME_LENGTH {
        return Err(ValidationError::TooLong("method", method.len()));
    }
    if let Some(ch) = token::has_wildcards_or_whitespace(method) {
        return Err(ValidationError::InvalidCharacter("method", ch));
    }
    if token::has_consecutive_or_boundary_dots(method) {
        return Err(ValidationError::InvalidCharacter("method", '.'));
    }
    Ok(())
}

fn validate_subject_capacity(prefix: &str) -> Result<(), ValidationError> {
    let max_prefix_session_subject_len =
        prefix.len() + 1 + MAX_SESSION_ID_LENGTH + MAX_SESSIONED_SUBJECT_SUFFIX_LEN;

    if max_prefix_session_subject_len > NATS_MAX_SUBJECT_BYTES {
        return Err(ValidationError::CombinedSubjectsTooLong(
            max_prefix_session_subject_len,
        ));
    }

    Ok(())
}

impl Config {
    pub fn new(acp_prefix: String, nats: NatsConfig) -> Result<Self, ValidationError> {
        validate_prefix(&acp_prefix)?;
        validate_subject_capacity(&acp_prefix)?;
        Ok(Self {
            acp_prefix,
            nats,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
            prompt_timeout: DEFAULT_PROMPT_TIMEOUT,
            max_concurrent_client_tasks: DEFAULT_MAX_CONCURRENT_CLIENT_TASKS,
            max_nats_payload_bytes: DEFAULT_NATS_MAX_PAYLOAD_BYTES,
        })
    }

    pub fn with_max_nats_payload_bytes(mut self, max_payload_bytes: usize) -> Self {
        self.max_nats_payload_bytes = max_payload_bytes.max(1);
        self
    }

    pub fn with_operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout;
        self
    }

    pub fn with_prompt_timeout(mut self, timeout: Duration) -> Self {
        self.prompt_timeout = timeout;
        self
    }

    pub fn acp_prefix(&self) -> &str {
        &self.acp_prefix
    }

    pub fn nats(&self) -> &NatsConfig {
        &self.nats
    }

    pub fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    /// Returns the configured timeout for prompt requests.
    pub fn prompt_timeout(&self) -> Duration {
        self.prompt_timeout
    }

    pub fn with_max_concurrent_client_tasks(mut self, max: usize) -> Self {
        self.max_concurrent_client_tasks = max.max(1);
        self
    }

    /// Returns the configured maximum number of concurrent client tasks.
    pub fn max_concurrent_client_tasks(&self) -> usize {
        self.max_concurrent_client_tasks
    }

    /// Returns the configured maximum response payload size for NATS messages.
    pub fn max_nats_payload_bytes(&self) -> usize {
        self.max_nats_payload_bytes
    }

    #[cfg(test)]
    pub(crate) fn for_test(acp_prefix: &str) -> Self {
        let nats = NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        };
        Self::new(acp_prefix.to_string(), nats).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_nats() -> NatsConfig {
        NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        }
    }

    #[test]
    fn config_new_valid_prefix() {
        assert!(Config::new("acp".to_string(), default_nats()).is_ok());
        assert!(Config::new("my.multi.part".to_string(), default_nats()).is_ok());
    }

    #[test]
    fn config_new_empty_prefix_returns_err() {
        let err = Config::new("".to_string(), default_nats()).err().unwrap();
        assert_eq!(err, ValidationError::EmptyValue("acp_prefix"));
    }

    #[test]
    fn config_new_wildcard_prefix_returns_err() {
        assert!(Config::new("acp.*".to_string(), default_nats()).is_err());
        assert!(Config::new("acp.>".to_string(), default_nats()).is_err());
    }

    #[test]
    fn config_new_whitespace_prefix_returns_err() {
        assert!(Config::new("acp prefix".to_string(), default_nats()).is_err());
        assert!(Config::new("acp\t".to_string(), default_nats()).is_err());
    }

    #[test]
    fn validate_method_name_valid() {
        assert!(validate_method_name("my_custom_method").is_ok());
        assert!(validate_method_name("simple").is_ok());
        assert!(validate_method_name("method123").is_ok());
    }

    #[test]
    fn validate_method_name_too_long_returns_err() {
        let long = "a".repeat(129);
        let err = validate_method_name(&long).err().unwrap();
        assert_eq!(err, ValidationError::TooLong("method", 129));
    }

    #[test]
    fn validate_method_name_dotted_namespaces_accepted() {
        assert!(validate_method_name("my.custom.method").is_ok());
        assert!(validate_method_name("a.b").is_ok());
        assert!(validate_method_name("vendor.operation").is_ok());
    }

    #[test]
    fn validate_method_name_malformed_dots_rejected() {
        assert!(validate_method_name("..method").is_err());
        assert!(validate_method_name("method..name").is_err());
        assert!(validate_method_name(".method").is_err());
        assert!(validate_method_name("method.").is_err());
        assert!(validate_method_name(".").is_err());
    }

    #[test]
    fn validate_method_name_empty_returns_err() {
        let err = validate_method_name("").err().unwrap();
        assert_eq!(err, ValidationError::EmptyValue("method"));
    }

    #[test]
    fn validate_method_name_wildcard_returns_err() {
        assert!(validate_method_name("method.*").is_err());
        assert!(validate_method_name("method.>").is_err());
    }

    #[test]
    fn validate_method_name_whitespace_returns_err() {
        assert!(validate_method_name("method name").is_err());
        assert!(validate_method_name("method\t").is_err());
    }

    #[test]
    fn validate_prefix_consecutive_dots_returns_err() {
        assert!(Config::new("acp..foo".to_string(), default_nats()).is_err());
        assert!(Config::new("foo..bar..baz".to_string(), default_nats()).is_err());
    }

    #[test]
    fn validate_prefix_boundary_dots_returns_err() {
        assert!(Config::new(".acp".to_string(), default_nats()).is_err());
        assert!(Config::new("acp.".to_string(), default_nats()).is_err());
        assert!(Config::new(".".to_string(), default_nats()).is_err());
    }

    #[test]
    fn config_new_too_long_prefix_returns_err() {
        let long_prefix = "a".repeat(129);
        let err = Config::new(long_prefix, default_nats())
            .err()
            .expect("acp prefix is too long");
        assert!(matches!(err, ValidationError::TooLong("acp_prefix", 129)));
    }

    #[test]
    fn config_new_too_long_prefix_for_session_subjects_returns_err() {
        let long_prefix = "a".repeat(93);
        let err = Config::new(long_prefix, default_nats())
            .err()
            .expect("combined session subject capacity should reject overlong prefix");
        assert!(
            matches!(err, ValidationError::CombinedSubjectsTooLong(_)),
            "Expected combined subject overflow validation error, got {err:?}"
        );
    }

    #[test]
    fn config_new_prefix_with_reserved_session_room_is_valid() {
        let max_compatible_prefix = "a".repeat(92);
        assert!(Config::new(max_compatible_prefix, default_nats()).is_ok());
    }

    #[test]
    fn config_with_max_concurrent_client_tasks_enforces_minimum() {
        let config = Config::new("acp".to_string(), default_nats())
            .expect("valid prefix");
        let config = config.with_max_concurrent_client_tasks(0);
        assert_eq!(config.max_concurrent_client_tasks(), 1);
    }

    #[test]
    fn max_sessioned_subject_suffix_len_covers_known_suffixes() {
        const KNOWN_SUFFIXES: [&str; 10] = [
            ".client.fs.read_text_file",
            ".client.fs.write_text_file",
            ".client.session.request_permission",
            ".client.session.update",
            ".client.terminal.create",
            ".client.terminal.kill",
            ".client.terminal.output",
            ".client.terminal.release",
            ".client.terminal.wait_for_exit",
            ".client.ext.session.prompt_response",
        ];

        let observed = KNOWN_SUFFIXES.iter().map(|s| s.len()).max().unwrap();
        assert_eq!(
            observed,
            MAX_SESSIONED_SUBJECT_SUFFIX_LEN,
            "If you add a new client subject suffix, update MAX_SESSIONED_SUBJECT_SUFFIX_LEN"
        );
    }
}

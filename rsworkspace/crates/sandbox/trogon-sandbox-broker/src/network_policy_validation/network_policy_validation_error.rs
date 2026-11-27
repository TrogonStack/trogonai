/// Why a user-defined network policy was refused before dispatch.
#[derive(Debug, thiserror::Error)]
pub enum NetworkPolicyValidationError {
    #[error("invalid regex '{pattern}': {source}")]
    InvalidRegex {
        pattern: String,
        #[source]
        source: regex::Error,
    },
}

use trogon_sandbox_contract::DomainPatternError;

/// Why an environment variable that configures the proxy could not be read.
#[derive(Debug, thiserror::Error)]
pub enum NetworkEnvError {
    #[error("{name} must be a positive number of seconds")]
    NotPositiveSeconds { name: &'static str },
    #[error("{name} must be a number of seconds")]
    NotSeconds { name: &'static str },
    #[error("{name} must be true or false")]
    NotBoolean { name: &'static str },
    #[error("{name} entry \"{entry}\": {source}")]
    InvalidHost {
        name: &'static str,
        entry: String,
        source: DomainPatternError,
    },
    #[error("{name} entry \"{entry}\" is not a CIDR range")]
    InvalidRange { name: &'static str, entry: String },
}

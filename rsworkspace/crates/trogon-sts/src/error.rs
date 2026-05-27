use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StsError {
    #[error("invalid_request: {0}")]
    InvalidRequest(String),
    #[error("invalid_grant: {0}")]
    InvalidGrant(String),
    #[error("invalid_target: {0}")]
    InvalidTarget(String),
    #[error("access_denied: {0}")]
    AccessDenied(String),
    #[error("act_chain_depth_exceeded")]
    ActChainDepthExceeded,
    #[error("act_chain_loop_detected")]
    ActChainLoopDetected,
    #[error("rate_limited: {0}")]
    RateLimited(String),
    #[error("registry_unavailable: {0}")]
    RegistryUnavailable(String),
    #[error("server_error: {0}")]
    ServerError(String),
}

impl StsError {
    #[must_use]
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::InvalidGrant(_) => "invalid_grant",
            Self::InvalidTarget(_) => "invalid_target",
            Self::AccessDenied(_) => "access_denied",
            Self::ActChainDepthExceeded => "act_chain_depth_exceeded",
            Self::ActChainLoopDetected => "act_chain_loop_detected",
            Self::RateLimited(_) => "rate_limited",
            Self::RegistryUnavailable(_) => "server_error",
            Self::ServerError(_) => "server_error",
        }
    }

    #[must_use]
    pub fn error_description(&self) -> String {
        self.to_string()
    }

    #[must_use]
    pub fn audit_outcome(&self) -> &'static str {
        match self {
            Self::RateLimited(_) => "rate_limit",
            Self::ServerError(_) | Self::RegistryUnavailable(_) => "error",
            _ => "deny",
        }
    }
}

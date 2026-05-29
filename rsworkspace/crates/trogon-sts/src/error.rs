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
    #[error("act_chain_entry_revoked: index {index} agent {agent_id}")]
    ActChainEntryRevoked { index: usize, agent_id: String },
    #[error("purpose_missing")]
    PurposeMissing,
    #[error("rate_limited: {0}")]
    RateLimited(String),
    #[error("registry_unavailable: {0}")]
    RegistryUnavailable(String),
    #[error("spicedb_unavailable: {0}")]
    SpiceDbUnavailable(String),
    #[error("dependency_unavailable: {0}")]
    DependencyUnavailable(String),
    #[error("signer_unavailable: {0}")]
    SignerUnavailable(String),
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
            Self::PurposeMissing => "invalid_request",
            Self::ActChainDepthExceeded => "act_chain_depth_exceeded",
            Self::ActChainLoopDetected => "act_chain_loop_detected",
            Self::ActChainEntryRevoked { .. } => "act_chain_entry_revoked",
            Self::RateLimited(_) => "rate_limited",
            Self::RegistryUnavailable(_) | Self::SpiceDbUnavailable(_) | Self::DependencyUnavailable(_)
            | Self::SignerUnavailable(_) | Self::ServerError(_) => "server_error",
        }
    }

    #[must_use]
    pub fn error_description(&self) -> String {
        self.to_string()
    }

    #[must_use]
    pub fn audit_reason(&self) -> String {
        match self {
            Self::RegistryUnavailable(_) => "registry_unavailable".into(),
            Self::SpiceDbUnavailable(_) => "spicedb_unavailable".into(),
            Self::DependencyUnavailable(_) => "dependency_unavailable".into(),
            Self::SignerUnavailable(_) => "signer_unavailable".into(),
            Self::ActChainEntryRevoked { .. } => "act_chain_entry_revoked".into(),
            Self::PurposeMissing => "purpose_missing".into(),
            other => other.error_description(),
        }
    }

    #[must_use]
    pub fn audit_outcome(&self) -> &'static str {
        match self {
            Self::RateLimited(_) => "rate_limit",
            Self::ServerError(_)
            | Self::RegistryUnavailable(_)
            | Self::SpiceDbUnavailable(_)
            | Self::DependencyUnavailable(_)
            | Self::SignerUnavailable(_) => "error",
            _ => "deny",
        }
    }

    #[must_use]
    pub fn dependency_audit_reason(&self) -> Option<&'static str> {
        match self {
            Self::DependencyUnavailable(_) => Some("dependency_unavailable"),
            Self::RegistryUnavailable(_) => Some("registry_unavailable"),
            Self::SpiceDbUnavailable(_) => Some("spicedb_unavailable"),
            _ => None,
        }
    }
}

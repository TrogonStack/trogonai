use std::time::Duration;

use crate::SensitiveString;
use crate::bridge_git::CodeSessionGitContext;

/// What a remote session is doing, as the bridge reports it to the service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Idle,
    Running,
    RequiresAction,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::RequiresAction => "requires_action",
        }
    }
}

/// MCP server settings the service attaches to a credentials reply. The field is undeclared upstream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteMcpConfig {
    pub content: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteCredentials {
    pub worker_jwt: SensitiveString,
    pub api_base_url: String,
    pub expires_in: f64,
    pub worker_epoch: i64,
    pub mcp_config: Option<RemoteMcpConfig>,
}

/// Why the service refused a trusted device token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceRefusal {
    TokenRefused,
    SignatureWanted { sub_reason: String },
    Unrecognized { sub_reason: String },
}

/// Which side of the edge produced a refusal, judged from the response headers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureSource {
    Origin,
    NonoriginCf,
    NonoriginOther,
}

impl FailureSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Origin => "origin",
            Self::NonoriginCf => "nonorigin_cf",
            Self::NonoriginOther => "nonorigin_other",
        }
    }
}

/// A credentials request that failed in a way that retrying with the same inputs cannot fix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialsFailure {
    InvalidSessionId,
    UntrustedDevice { refusal: DeviceRefusal },
    SessionStaleRelogin,
    SessionSecurityTierBelowOrgPolicy,
    RequestRejected { status: u16, source: Option<FailureSource> },
    MalformedResponse { status: u16 },
}

impl CredentialsFailure {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::InvalidSessionId => "invalid_session_id",
            Self::UntrustedDevice { .. } => "untrusted_device",
            Self::SessionStaleRelogin => "session_stale_relogin",
            Self::SessionSecurityTierBelowOrgPolicy => "session_security_tier_below_org_policy",
            Self::RequestRejected { .. } => "request_rejected",
            Self::MalformedResponse { .. } => "malformed_response",
        }
    }

    pub fn terminal(&self) -> bool {
        true
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::SessionSecurityTierBelowOrgPolicy => Some(403),
            Self::RequestRejected { status, .. } | Self::MalformedResponse { status } => Some(*status),
            _ => None,
        }
    }

    pub fn source(&self) -> Option<FailureSource> {
        match self {
            Self::RequestRejected { source, .. } => *source,
            _ => None,
        }
    }
}

/// The service rejected the access token. Refreshing the token and asking again can succeed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CredentialsRejection;

impl CredentialsRejection {
    pub fn reason(&self) -> &'static str {
        "oauth_rejected"
    }

    pub fn terminal(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateSessionFailure {
    GroupingRejected { status: u16, detail: Option<String> },
    RequestRejected { status: u16, detail: Option<String> },
    MalformedResponse { status: u16 },
}

impl CreateSessionFailure {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::GroupingRejected { .. } => "grouping_rejected",
            Self::RequestRejected { .. } => "request_rejected",
            Self::MalformedResponse { .. } => "malformed_response",
        }
    }

    pub fn terminal(&self) -> bool {
        true
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::GroupingRejected { status, .. }
            | Self::RequestRejected { status, .. }
            | Self::MalformedResponse { status } => *status,
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::GroupingRejected { detail, .. } | Self::RequestRejected { detail, .. } => detail.as_deref(),
            Self::MalformedResponse { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RemoteCredentialsOutcome {
    Credentials(RemoteCredentials),
    Failure(CredentialsFailure),
    Rejected(CredentialsRejection),
    /// Network error, timeout, 5xx, 408 or 429: worth retrying.
    Unavailable,
}

impl RemoteCredentialsOutcome {
    pub fn is_credentials_failure(&self) -> bool {
        matches!(self, Self::Failure(_))
    }

    pub fn is_credentials_rejection(&self) -> bool {
        matches!(self, Self::Rejected(_))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateSessionOutcome {
    Created(String),
    Failure(CreateSessionFailure),
    Rejected(CredentialsRejection),
    Unavailable,
}

impl CreateSessionOutcome {
    pub fn is_create_session_failure(&self) -> bool {
        matches!(self, Self::Failure(_))
    }

    pub fn is_credentials_rejection(&self) -> bool {
        matches!(self, Self::Rejected(_))
    }
}

#[derive(Clone, Debug)]
pub struct FetchRemoteCredentialsOptions {
    pub session_id: String,
    pub base_url: String,
    pub access_token: SensitiveString,
    pub timeout: Option<Duration>,
    pub trusted_device_token: Option<SensitiveString>,
}

#[derive(Clone, Debug)]
pub struct CreateCodeSessionOptions {
    pub base_url: String,
    pub access_token: SensitiveString,
    pub title: String,
    pub timeout: Option<Duration>,
    pub tags: Vec<String>,
    pub git_context: Option<CodeSessionGitContext>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub session_grouping_id: Option<String>,
}

#[cfg(test)]
#[path = "bridge_types_tests.rs"]
mod tests;

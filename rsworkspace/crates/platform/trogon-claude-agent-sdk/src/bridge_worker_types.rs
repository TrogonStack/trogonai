use std::fmt;

use serde_json::{Map, Value};

/// Why the worker service refused to keep this worker, as the upstream request path names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MismatchReason {
    EpochConflict,
    SupersededByWorker,
    SessionNotActive,
    OrganizationMovedAway,
    EpochStale,
    SessionNotFound,
    TokenExpired,
    AuthExhausted,
}

impl MismatchReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EpochConflict => "epoch_conflict",
            Self::SupersededByWorker => "superseded_by_worker",
            Self::SessionNotActive => "session_not_active",
            Self::OrganizationMovedAway => "organization_moved_away",
            Self::EpochStale => "epoch_stale",
            Self::SessionNotFound => "session_not_found",
            Self::TokenExpired => "token_expired",
            Self::AuthExhausted => "auth_exhausted",
        }
    }

    /// The reason a `409` names in its `x-ccr-conflict-reason` header or its body. Only these four are accepted; any
    /// other text is read as no reason.
    pub fn from_conflict(text: &str) -> Option<Self> {
        match text {
            "superseded_by_worker" => Some(Self::SupersededByWorker),
            "epoch_stale" => Some(Self::EpochStale),
            "session_not_active" => Some(Self::SessionNotActive),
            "organization_moved_away" => Some(Self::OrganizationMovedAway),
            _ => None,
        }
    }

    /// The WebSocket-style close code the upstream table gives the reason: `4094` asks for auth recovery, `4090`
    /// for a poll-loop restart. `typed` is `causeTypedCloseCodes`; without it the attach entrypoint always uses `4090`.
    pub fn close_code(self, typed: bool) -> u16 {
        match self {
            Self::TokenExpired | Self::AuthExhausted if typed => 4094,
            _ => 4090,
        }
    }

    /// Whether a fresh worker could still take the session over after this reason.
    pub fn retryable(self) -> bool {
        !matches!(self, Self::SessionNotFound | Self::TokenExpired | Self::AuthExhausted)
    }
}

/// The detail the service gets when the worker asks to be parked on a permission prompt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequiresActionDetails {
    pub tool_name: String,
    pub display_tool_name: Option<String>,
    pub action_description: String,
    pub raw_command: Option<String>,
    pub request_id: String,
    pub tool_use_id: String,
}

impl RequiresActionDetails {
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("tool_name".into(), Value::String(self.tool_name.clone()));
        if let Some(name) = &self.display_tool_name {
            map.insert("display_tool_name".into(), Value::String(name.clone()));
        }
        map.insert(
            "action_description".into(),
            Value::String(self.action_description.clone()),
        );
        if let Some(command) = &self.raw_command {
            map.insert("raw_command".into(), Value::String(command.clone()));
        }
        map.insert("request_id".into(), Value::String(self.request_id.clone()));
        map.insert("tool_use_id".into(), Value::String(self.tool_use_id.clone()));
        Value::Object(map)
    }
}

/// How `registerWorker` failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisterFailure {
    Status(u16),
    TimedOut,
    Unreachable,
    InvalidEpoch { detail: Option<String> },
}

impl RegisterFailure {
    /// The text of the error the upstream call throws.
    pub fn message(&self) -> String {
        match self {
            Self::Status(status) => format!("Request failed with status code {status}"),
            Self::TimedOut => "timeout of 10000ms exceeded".to_owned(),
            Self::Unreachable => "Network Error".to_owned(),
            Self::InvalidEpoch { detail: Some(detail) } => {
                format!("registerWorker: invalid worker_epoch in response: {detail}")
            }
            Self::InvalidEpoch { detail: None } => "registerWorker: invalid worker_epoch in response (the response is \
                                                     not quoted: it is too long or is not one plain line)"
                .to_owned(),
        }
    }

    /// The axios error code the upstream error carries, when it has one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Status(status) if status / 100 == 4 => Some("ERR_BAD_REQUEST"),
            Self::Status(_) => Some("ERR_BAD_RESPONSE"),
            Self::TimedOut => Some("ECONNABORTED"),
            Self::Unreachable => Some("ERR_NETWORK"),
            Self::InvalidEpoch { .. } => None,
        }
    }
}

impl fmt::Display for RegisterFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for RegisterFailure {}

/// The `CCRClient init failed` error: the worker could not register its first state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitFailure {
    pub reason: &'static str,
    pub http_status: Option<u16>,
    pub no_answer: bool,
}

impl fmt::Display for InitFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "CCRClient init failed: {}", self.reason)
    }
}

impl std::error::Error for InitFailure {}

/// An init that stopped: the service refused the registration, or the request path escalated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InitError {
    Failed(InitFailure),
    Mismatch(MismatchReason),
}

/// Why one request did not get an accepted answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestFailure {
    pub status: Option<u16>,
    pub reason: String,
    pub retry_after_ms: Option<f64>,
    pub fetch_failed: bool,
}

/// What one request to the worker service came to.
#[derive(Clone, Debug, PartialEq)]
pub enum RequestOutcome {
    /// The service accepted the request; the body is kept only when the caller asked for it and it parsed.
    Accepted(Option<Value>),
    Failed(RequestFailure),
}

impl RequestOutcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted(_))
    }
}

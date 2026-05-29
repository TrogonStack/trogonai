//! Typed errors for NATS-callout plugin request/reply.

use std::fmt;

/// Failure classification for deny-bias handling per failure-mode matrix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginFailureClass {
    /// Timeout or transport failure — treat as transient; drop request (deny-bias).
    Transient,
    /// Malformed or protocol-mismatched reply — permanent; drop request.
    Permanent,
}

/// Errors surfaced by [`super::dispatcher::PluginDispatcher::invoke`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginCalloutError {
    /// Plugin did not reply within the configured deadline (default 250 ms).
    Timeout,
    /// NATS request/reply transport failed (connection, publish, etc.).
    Transport(String),
    /// Reply bytes are not valid JSON or lack required decision fields.
    MalformedReply(String),
    /// Reply JSON parses but decision shape is invalid (unknown verb, missing fields).
    ProtocolMismatch(String),
}

impl PluginCalloutError {
    #[must_use]
    pub fn failure_class(&self) -> PluginFailureClass {
        match self {
            Self::Timeout | Self::Transport(_) => PluginFailureClass::Transient,
            Self::MalformedReply(_) | Self::ProtocolMismatch(_) => PluginFailureClass::Permanent,
        }
    }

    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self.failure_class(), PluginFailureClass::Transient)
    }

    #[must_use]
    pub fn audit_failure_reason(&self) -> &'static str {
        match self.failure_class() {
            PluginFailureClass::Transient => "transient",
            PluginFailureClass::Permanent => "permanent",
        }
    }
}

impl fmt::Display for PluginCalloutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("plugin callout timed out"),
            Self::Transport(msg) => write!(f, "plugin callout transport error: {msg}"),
            Self::MalformedReply(msg) => write!(f, "plugin callout malformed reply: {msg}"),
            Self::ProtocolMismatch(msg) => write!(f, "plugin callout protocol mismatch: {msg}"),
        }
    }
}

impl std::error::Error for PluginCalloutError {}

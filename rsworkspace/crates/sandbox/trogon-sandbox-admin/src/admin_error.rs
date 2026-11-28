use std::error::Error;
use std::io;

/// A failure of one admin command.
#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    /// A backend or store call failed. `context` names the step that was under way.
    #[error("{context}: {source}")]
    Step {
        context: &'static str,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("write output: {0}")]
    Output(#[source] io::Error),
    #[error("encode output: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("parse {what}: {source}")]
    Parse {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("api key record {key} does not parse: {source}")]
    UndecodableApiKey {
        key: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("pattern must not be empty")]
    EmptyPattern,
    #[error("pattern '{pattern}' is invalid: wildcards are only allowed as a trailing suffix")]
    MisplacedWildcard { pattern: String },
    #[error("prefix must not be empty")]
    EmptyHostMountPrefix,
    #[error("prefix '{prefix}' is invalid: must be an absolute path starting with '/'")]
    RelativeHostMountPrefix { prefix: String },
    #[error("unknown artifact type '{value}'")]
    UnknownArtifactType { value: String },
    #[error("month must be in YYYY-MM format: {0}")]
    InvalidMonth(#[source] chrono::ParseError),
    #[error("the broker is still settling this freeze ({settle_attempts} attempts so far); pass --force to act anyway")]
    FreezeStillSettling { settle_attempts: u32 },
    #[error("the lease kept changing; run the command again")]
    FreezeConflict,
    #[error("no live worker heartbeat for {provider}")]
    NoLiveWorker { provider: String },
    #[error("tag {tenant}/{name} not found")]
    TagNotFound { tenant: String, name: String },
    #[error("revision conflict after {attempts} attempts; re-run with a current --expected-revision")]
    TagRevisionConflict { attempts: u8 },
    #[error("tag clear failed after {attempts} attempts")]
    TagClearExhausted { attempts: u8 },
    #[error("no capture status recorded for lease {lease_id}")]
    NoCaptureStatus { lease_id: String },
    #[error("{failed} credential(s) failed to migrate")]
    MigrationFailed { failed: usize },
    #[error("{remaining} legacy credential(s) remain")]
    LegacyRemain { remaining: usize },
}

impl AdminError {
    /// Wraps a failure of the step `context` so `?` can carry it out of a command.
    pub(crate) fn step<E>(context: &'static str) -> impl FnOnce(E) -> Self
    where
        E: Into<Box<dyn Error + Send + Sync>>,
    {
        move |source| Self::Step {
            context,
            source: source.into(),
        }
    }
}

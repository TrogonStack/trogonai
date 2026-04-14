use std::fmt;

/// Errors returned by [`crate::ActorRuntime::handle_event`].
///
/// Parameterised by `E`, the actor's own error type, so the actor's failures
/// are surfaced with full type information rather than being boxed.
#[derive(Debug)]
pub enum ActorError<E: std::error::Error> {
    /// Actor's `handle` / `on_create` / `on_destroy` returned an error.
    Actor(E),
    /// Could not load or save actor state from the KV store.
    State(String),
    /// Could not serialize actor state to JSON.
    Serialize(serde_json::Error),
    /// Could not deserialize actor state from JSON.
    Deserialize(serde_json::Error),
    /// Could not write an entry to the transcript.
    Transcript(String),
    /// `spawn_agent` failed (network error, timeout, …).
    SpawnFailed(String),
    /// `spawn_agent` was called for a capability no registered agent provides.
    NoAgentFound(String),
    /// Optimistic concurrency retry limit exceeded.
    RetryLimitExceeded,
}

impl<E: std::error::Error> fmt::Display for ActorError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActorError::Actor(e) => write!(f, "actor error: {e}"),
            ActorError::State(msg) => write!(f, "state store error: {msg}"),
            ActorError::Serialize(e) => write!(f, "state serialization error: {e}"),
            ActorError::Deserialize(e) => write!(f, "state deserialization error: {e}"),
            ActorError::Transcript(msg) => write!(f, "transcript error: {msg}"),
            ActorError::SpawnFailed(msg) => write!(f, "sub-agent spawn failed: {msg}"),
            ActorError::NoAgentFound(cap) => {
                write!(f, "no agent found with capability '{cap}'")
            }
            ActorError::RetryLimitExceeded => {
                write!(f, "optimistic concurrency retry limit exceeded")
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ActorError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ActorError::Actor(e) => Some(e),
            ActorError::Serialize(e) | ActorError::Deserialize(e) => Some(e),
            _ => None,
        }
    }
}

/// Outcome of a state-save attempt in the runtime's optimistic-concurrency loop.
#[derive(Debug)]
pub enum SaveError {
    /// The revision we read is no longer current — another event landed first.
    /// The runtime will reload state and retry.
    Conflict,
    /// A genuine storage or network error — do not retry.
    Other(String),
}

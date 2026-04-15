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

#[cfg(test)]
mod tests {
    use super::*;

    fn json_err() -> serde_json::Error {
        serde_json::from_str::<u32>("{bad}").unwrap_err()
    }

    #[test]
    fn display_actor() {
        let e = ActorError::Actor(std::io::Error::new(std::io::ErrorKind::Other, "boom"));
        assert!(format!("{e}").contains("actor error"));
    }

    #[test]
    fn display_state() {
        assert!(format!("{}", ActorError::<std::io::Error>::State("oops".into()))
            .contains("state store error"));
    }

    #[test]
    fn display_serialize() {
        assert!(format!("{}", ActorError::<std::io::Error>::Serialize(json_err()))
            .contains("serialization error"));
    }

    #[test]
    fn display_deserialize() {
        assert!(format!("{}", ActorError::<std::io::Error>::Deserialize(json_err()))
            .contains("deserialization error"));
    }

    #[test]
    fn display_transcript() {
        assert!(format!("{}", ActorError::<std::io::Error>::Transcript("t".into()))
            .contains("transcript error"));
    }

    #[test]
    fn display_spawn_failed() {
        assert!(format!("{}", ActorError::<std::io::Error>::SpawnFailed("net".into()))
            .contains("spawn failed"));
    }

    #[test]
    fn display_no_agent_found() {
        assert!(format!("{}", ActorError::<std::io::Error>::NoAgentFound("cap".into()))
            .contains("no agent found"));
    }

    #[test]
    fn display_retry_limit_exceeded() {
        assert!(format!("{}", ActorError::<std::io::Error>::RetryLimitExceeded)
            .contains("retry limit"));
    }

    #[test]
    fn source_actor_is_some() {
        use std::error::Error;
        let e = ActorError::Actor(std::io::Error::new(std::io::ErrorKind::Other, "boom"));
        assert!(e.source().is_some());
    }

    #[test]
    fn source_serialize_is_some() {
        use std::error::Error;
        let e = ActorError::<std::io::Error>::Serialize(json_err());
        assert!(e.source().is_some());
    }

    #[test]
    fn source_state_is_none() {
        use std::error::Error;
        let e = ActorError::<std::io::Error>::State("msg".into());
        assert!(e.source().is_none());
    }
}

/// Outcome of a state-save attempt in the runtime's optimistic-concurrency loop.
#[derive(Debug)]
pub enum SaveError {
    /// The revision we read is no longer current — another event landed first.
    /// The runtime will reload state and retry.
    Conflict,
    /// A genuine storage or network error — do not retry.
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Conflict => write!(f, "optimistic concurrency conflict"),
            SaveError::Other(e) => write!(f, "state save error: {e}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SaveError::Other(e) => Some(e.as_ref()),
            SaveError::Conflict => None,
        }
    }
}

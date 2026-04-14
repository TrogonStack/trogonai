use std::fmt;

#[derive(Debug)]
pub enum RouterError {
    /// Failed to subscribe to the event subject.
    Subscribe(String),
    /// Failed to publish to an actor's inbox.
    Publish(String),
    /// Registry query failed.
    Registry(String),
    /// The LLM HTTP request failed.
    LlmRequest(String),
    /// The LLM returned a response that could not be parsed.
    LlmParse(String),
    /// The routing decision referenced an agent type not in the registry.
    UnknownAgentType(String),
    /// Transcript append error (non-fatal in most cases — logged but not propagated).
    Transcript(String),
}

impl fmt::Display for RouterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouterError::Subscribe(msg) => write!(f, "subscribe error: {msg}"),
            RouterError::Publish(msg) => write!(f, "publish error: {msg}"),
            RouterError::Registry(msg) => write!(f, "registry error: {msg}"),
            RouterError::LlmRequest(msg) => write!(f, "LLM request error: {msg}"),
            RouterError::LlmParse(msg) => write!(f, "LLM response parse error: {msg}"),
            RouterError::UnknownAgentType(t) => {
                write!(f, "routing decision references unknown agent type '{t}'")
            }
            RouterError::Transcript(msg) => write!(f, "transcript error: {msg}"),
        }
    }
}

impl std::error::Error for RouterError {}

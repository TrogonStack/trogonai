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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_subscribe() {
        assert!(format!("{}", RouterError::Subscribe("x".into())).contains("subscribe error"));
    }

    #[test]
    fn display_publish() {
        assert!(format!("{}", RouterError::Publish("x".into())).contains("publish error"));
    }

    #[test]
    fn display_registry() {
        assert!(format!("{}", RouterError::Registry("x".into())).contains("registry error"));
    }

    #[test]
    fn display_llm_request() {
        assert!(format!("{}", RouterError::LlmRequest("x".into())).contains("LLM request error"));
    }

    #[test]
    fn display_llm_parse() {
        assert!(
            format!("{}", RouterError::LlmParse("x".into())).contains("LLM response parse error")
        );
    }

    #[test]
    fn display_unknown_agent_type() {
        assert!(
            format!("{}", RouterError::UnknownAgentType("Ghost".into()))
                .contains("unknown agent type")
        );
    }

    #[test]
    fn display_transcript() {
        assert!(format!("{}", RouterError::Transcript("x".into())).contains("transcript error"));
    }
}

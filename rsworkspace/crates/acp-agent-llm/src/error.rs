use crate::model_id::ModelId;
use crate::provider_name::ProviderName;

/// Errors from the LLM agent that map to ACP error responses.
#[derive(Debug)]
pub enum LlmAgentError {
    SessionNotFound(String),
    ProviderNotConfigured(ProviderName),
    ModelNotFound(ModelId),
    CompletionError(CompletionError),
    NoTextContent,
}

impl std::fmt::Display for LlmAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(f, "session not found: {id}"),
            Self::ProviderNotConfigured(name) => write!(f, "provider not configured: {name}"),
            Self::ModelNotFound(id) => write!(f, "model not found: {id}"),
            Self::CompletionError(e) => write!(f, "completion error: {e}"),
            Self::NoTextContent => f.write_str("prompt contains no text content"),
        }
    }
}

impl std::error::Error for LlmAgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CompletionError(e) => Some(e),
            _ => None,
        }
    }
}

/// Errors from an LLM provider HTTP call.
#[derive(Debug)]
pub enum CompletionError {
    Http(reqwest::Error),
    Api { status: u16, message: String },
    StreamParse(String),
    Cancelled,
}

impl std::fmt::Display for CompletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Api { status, message } => write!(f, "API error ({status}): {message}"),
            Self::StreamParse(msg) => write!(f, "stream parse error: {msg}"),
            Self::Cancelled => f.write_str("request cancelled"),
        }
    }
}

impl std::error::Error for CompletionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            _ => None,
        }
    }
}

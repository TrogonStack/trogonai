use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactorError {
    #[error("HTTP error calling summarization LLM: {0}")]
    Http(String),

    #[error("summarization LLM returned an empty response")]
    EmptyResponse,

    #[error("summarization LLM returned unexpected stop reason: {0}")]
    UnexpectedStopReason(String),

    #[error("invalid compaction request: {0}")]
    InvalidRequest(String),
}

impl From<reqwest::Error> for CompactorError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e.to_string())
    }
}

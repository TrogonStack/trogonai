use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactorError {
    #[error("HTTP error calling summarization LLM: {0}")]
    Http(#[from] reqwest::Error),

    #[error("summarization LLM returned an empty response")]
    EmptyResponse,

    #[error("summarization LLM returned unexpected stop reason: {0}")]
    UnexpectedStopReason(String),
}

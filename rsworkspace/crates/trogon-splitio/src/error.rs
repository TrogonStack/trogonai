use thiserror::Error;

#[derive(Debug, Error)]
pub enum SplitError {
    #[error("HTTP transport error: {0}")]
    Http(String),

    #[error("Evaluator returned status {status}: {body}")]
    EvaluatorError { status: u16, body: String },

    #[error("Unexpected response shape: {0}")]
    UnexpectedResponse(String),
}

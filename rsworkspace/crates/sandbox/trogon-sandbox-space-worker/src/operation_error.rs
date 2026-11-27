/// Why an operation request could not be answered.
#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    /// The request body is not the JSON the verb takes.
    #[error("operation request or reply is not valid JSON for its verb: {0}")]
    Payload(#[from] serde_json::Error),
    /// The lease ID in the body differs from the one in the subject.
    #[error("subject and payload lease IDs disagree")]
    LeaseMismatch,
}

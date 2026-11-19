#[derive(Debug, thiserror::Error)]
pub enum WireSchemaError {
    #[error("descriptor JSON is malformed")]
    Json(#[from] serde_json::Error),
    #[error("descriptor metadata is invalid or exceeds its bound")]
    InvalidDescriptor,
    #[error("descriptor syntax is unsupported")]
    UnsupportedSyntax,
    #[error("descriptor names are duplicated")]
    DuplicateName,
    #[error("descriptor references a missing type")]
    MissingType,
}

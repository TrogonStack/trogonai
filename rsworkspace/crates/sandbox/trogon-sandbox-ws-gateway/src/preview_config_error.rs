/// Why the preview configuration was refused.
#[derive(Debug, thiserror::Error)]
pub enum PreviewConfigError {
    #[error("no preview signing keys configured")]
    Empty,
    #[error("invalid preview signing key entry: expected `kid=hexkey`")]
    MissingSeparator,
    #[error("preview signing key id `{kid}` must be 1-32 chars of [A-Za-z0-9_-]")]
    InvalidKeyId { kid: String },
    #[error("preview signing key `{kid}` is not valid hex")]
    InvalidHex { kid: String },
    #[error("preview signing key `{kid}` is shorter than 32 bytes")]
    KeyTooShort { kid: String },
    #[error("duplicate preview signing key id `{kid}`")]
    DuplicateKeyId { kid: String },
    #[error("active preview key id `{kid}` is not among the keys")]
    UnknownActiveKey { kid: String },
    #[error("environment variable `{name}` is not valid Unicode")]
    NotUnicode { name: &'static str },
    #[error("a public URL is required when preview signing keys are set")]
    MissingPublicUrl,
    #[error("preview http client could not be built: {0}")]
    Client(#[from] reqwest::Error),
}

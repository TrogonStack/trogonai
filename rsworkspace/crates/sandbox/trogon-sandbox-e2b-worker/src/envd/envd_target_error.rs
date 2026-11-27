/// Why an envd address could not be built.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum EnvdTargetError {
    #[error("the envd address is not a valid URL: {0}")]
    InvalidAddress(#[from] url::ParseError),
}

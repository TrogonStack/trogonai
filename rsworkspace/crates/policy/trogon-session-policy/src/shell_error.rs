use crate::Denial;

#[derive(Debug, thiserror::Error)]
pub enum ShellError<E: std::error::Error, R: std::error::Error, A: std::error::Error> {
    #[error("session policy refused the operation: {0:?}")]
    Refused(Denial),
    #[error("the denial could not be committed")]
    Audit(#[source] A),
    #[error("the reviewer was unavailable")]
    Review(#[source] R),
    #[error("the admitted effect failed; do not automatically retry")]
    Effect(#[source] E),
}

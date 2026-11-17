/// Why a runtime service id cannot be the principal of a release subject.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RuntimeServiceError {
    #[error("runtime service id is not carried verbatim by a subject token")]
    NotVerbatim,
}

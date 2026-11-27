/// Why a subject is not a provider operation subject.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("{0:?} is not a sandbox.provider.space.<verb>.<lease> subject")]
pub struct OperationSubjectError(pub String);

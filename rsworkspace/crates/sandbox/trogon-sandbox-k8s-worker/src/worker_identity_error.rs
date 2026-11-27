/// Why a worker identity is not a Kubernetes label value.
#[derive(Debug, thiserror::Error)]
#[error("{0:?} is not a valid Kubernetes label value")]
pub struct WorkerIdentityError(pub String);

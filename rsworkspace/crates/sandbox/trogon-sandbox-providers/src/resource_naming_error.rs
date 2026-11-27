/// Why a string is not a Kubernetes resource naming.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum K8sResourceNamingError {
    #[error("resource naming must be sandbox or legacy, got {0:?}")]
    Unknown(String),
}

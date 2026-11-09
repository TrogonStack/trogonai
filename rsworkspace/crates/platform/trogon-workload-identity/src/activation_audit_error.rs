#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ActivationAuditError {
    #[error("credential activation audit is unavailable")]
    Unavailable,
}

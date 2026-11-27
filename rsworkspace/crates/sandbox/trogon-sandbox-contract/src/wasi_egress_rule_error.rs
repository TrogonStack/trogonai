/// A host is not one a [`WasiEgressRule`](crate::WasiEgressRule) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum WasiEgressRuleError {
    #[error("host must not be empty")]
    EmptyHost,
}

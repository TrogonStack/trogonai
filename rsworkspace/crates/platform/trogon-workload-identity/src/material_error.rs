#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MaterialError {
    #[error("workload material exceeds resource bounds")]
    Bounds,
    #[error("Workload API CRL material requires an unsupported verification profile")]
    RevocationUnsupported,
    #[error("the exact workload identity is absent or ambiguous")]
    Identity,
    #[error("workload certificate profile or validity is invalid")]
    Profile,
    #[error("workload certificate path is invalid")]
    Certificate,
    #[error("workload certificate and private key do not match")]
    Key,
    #[error("workload material or peer registration is unavailable")]
    Unavailable,
}

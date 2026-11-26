use crate::DispatchMalformedReason;

/// Why a signed dispatch or per-lease request was refused.
#[derive(Debug, thiserror::Error)]
pub enum DispatchAuthError {
    #[error("dispatch is not signed")]
    Missing,
    #[error("dispatch signature is malformed: {0}")]
    Malformed(DispatchMalformedReason),
    #[error("dispatch signed with unknown key {0}")]
    UnknownKey(String),
    #[error("dispatch signature does not verify")]
    BadSignature,
    #[error("dispatch signature has expired")]
    Expired,
    #[error("dispatch signed for operation {found}, expected {expected}")]
    WrongOperation { expected: String, found: String },
    #[error("dispatch signed for worker {found}, this worker is {expected}")]
    WrongWorker { expected: String, found: String },
    #[error("dispatch body differs from what was signed")]
    BodyMismatch,
    #[error("dispatch tenant differs from the signed tenant")]
    TenantMismatch,
    #[error("lease id is already held for a different tenant")]
    LeaseTenantConflict,
    /// The signed lease differs from the one in the subject or body.
    #[error("signed lease differs from the request's lease")]
    LeaseMismatch,
    /// The request is for an attempt that is not the live one for its lease.
    #[error("request is for an attempt that is not the live one for this lease")]
    StaleAttempt,
    /// The request carries an older lease epoch than the one already held.
    #[error("request carries an older lease epoch than the one held for this lease")]
    StaleEpoch,
    /// The same signed request was already seen.
    #[error("signed request was already seen")]
    Replayed,
}

impl From<DispatchMalformedReason> for DispatchAuthError {
    fn from(reason: DispatchMalformedReason) -> Self {
        Self::Malformed(reason)
    }
}

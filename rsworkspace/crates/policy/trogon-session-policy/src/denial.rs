use crate::{DenialReason, Operation, PolicyFingerprint, SessionId};

/// A denial receipt for a protected local audit sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    pub session: SessionId,
    pub policy: PolicyFingerprint,
    pub operation: Operation,
    pub reason: DenialReason,
}

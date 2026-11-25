//! Operator-facing events. Each carries a stable `reason` code and never the
//! certificates, private keys or bundle contents that caused it.

use crate::source_fault::SourceFault;

/// `repeated` is true when the previous attempt failed for the same reason, so
/// a source that stays down logs once at `warn` and then at `debug`.
pub(crate) fn source_fault(fault: SourceFault, repeated: bool) {
    if repeated {
        tracing::debug!(reason = fault.code(), "workload source still failing");
    } else {
        tracing::warn!(reason = fault.code(), "workload source failed");
    }
}

pub(crate) fn source_withdrawn() {
    tracing::warn!(reason = "source_withdrawn", "workload material withdrawn");
}

/// A JWT-SVID was refused. Carries the reason code only, never the token, its
/// claims or any key.
pub(crate) fn jwt_svid_refused(reason: &'static str) {
    tracing::warn!(reason, "jwt-svid refused");
}

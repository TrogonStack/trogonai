use crate::probe_outcome::ProbeOutcome;

/// One outcome per attempt of `fence-helper wfp verify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WfpVerifyProbes {
    pub plain: ProbeOutcome,
    pub anonymous: ProbeOutcome,
    pub anonymous_loopback: ProbeOutcome,
    pub deny_only: ProbeOutcome,
    pub low_integrity: ProbeOutcome,
    pub low_integrity_loopback: ProbeOutcome,
    pub untrusted_integrity: ProbeOutcome,
    pub untrusted_integrity_loopback: ProbeOutcome,
    /// With loopback open: own token, to the loopback listener.
    pub open_loopback: Option<ProbeOutcome>,
}

use std::time::Duration;

/// Why an egress sidecar did not confirm that its firewall was in place.
#[derive(Debug, thiserror::Error)]
pub enum EgressRulesFailure {
    #[error("the egress sidecar exited with code {code} before its firewall was in place")]
    SidecarExited { code: i32 },
    #[error("the egress sidecar stopped before its firewall was in place: {state}")]
    SidecarFailed { state: String },
    #[error("the egress sidecar did not confirm its firewall within {waited:?}")]
    TimedOut { waited: Duration },
}

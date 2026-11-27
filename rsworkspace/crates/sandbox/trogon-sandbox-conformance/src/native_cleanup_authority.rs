use serde::Serialize;

/// How much a recorded native grant proves about the request that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeCleanupAuthority {
    /// The grant names the exact worker, attempt and epoch the request was assigned to.
    RequestMatched,
    /// The request named no worker, so the grant must be reconciled before it is cleaned up.
    ReconciliationRequired,
}

//! SpiceDB — gateway **`BulkCheckPermission`** seam for ingress / catalog shaping.
//!
//! Production wiring connects to the org-standard SpiceDB cluster and wraps the platform
//! **`zed`** watch API client (Authzed terminology only — no SpiceDB crate dependency here).
//!
//! **Catalog shaping:** discovery endpoints bulk-check `view` on a stable agent id list so
//! results reuse a per-session **ZedToken** cache instead of racing scatter-gather liveness.
//!
//! **Consistency:** callers pass a [`ConsistencyHint`] derived from the session cache; responses
//! return fresh tokens via [`BulkCheckOutcome::zed_token`] for the next check.

use std::error::Error;
use std::future::Future;

/// Session-scoped ZedToken consistency carrier passed into bulk checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsistencyHint {
    pub zed_token: String,
}

impl ConsistencyHint {
    pub fn minimize_latency() -> Self {
        Self {
            zed_token: String::new(),
        }
    }
}

/// One permission decision from a bulk check (placeholder until real tuple wiring lands).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BulkCheckOutcome {
    pub allow: bool,
    pub zed_token: String,
}

/// Bulk permission evaluation for catalog shaping and ingress authorization.
pub trait BulkPermissionCheck: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    fn bulk_check(
        &self,
        consistency: ConsistencyHint,
        resource_ids: &[String],
    ) -> impl Future<Output = Result<Vec<BulkCheckOutcome>, Self::Error>> + Send;
}

/// Per-session ZedToken cache scoped to a gateway caller session.
pub trait ZedTokenCache: Send + Sync {
    fn cached_hint(&self, session_key: &str) -> Option<ConsistencyHint>;

    fn record_hint(&self, session_key: &str, hint: ConsistencyHint);
}

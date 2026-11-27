use tokio::time::Instant;
use trogon_sandbox_contract::Digest;

use crate::WarmCandidate;

/// A warm VM the pool tracks, and since when it has been idle.
pub(crate) struct WarmEntry {
    pub(crate) digest: Digest,
    pub(crate) candidate: WarmCandidate,
    pub(crate) idle_since: Instant,
}

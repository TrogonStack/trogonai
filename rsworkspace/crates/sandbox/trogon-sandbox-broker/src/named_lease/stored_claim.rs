use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::LeaseId;

use super::NamedLeaseStoreError;

/// What the KV bucket stores under a name. A confirmed mapping keeps the bare
/// lease id encoding older brokers wrote; a pending claim adds a marker with
/// the wall-clock time (ms since the epoch) after which it no longer blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum StoredClaim {
    Confirmed(LeaseId),
    Pending { lease_id: LeaseId, pending_until_ms: u64 },
}

impl StoredClaim {
    pub(super) fn pending(lease_id: &LeaseId, ttl: Duration) -> Self {
        Self::Pending {
            lease_id: lease_id.clone(),
            pending_until_ms: unix_ms(SystemTime::now() + ttl),
        }
    }

    pub(super) fn lease(&self) -> &LeaseId {
        match self {
            Self::Confirmed(lease) | Self::Pending { lease_id: lease, .. } => lease,
        }
    }

    pub(super) fn is_pending(&self, now: SystemTime) -> bool {
        matches!(self, Self::Pending { pending_until_ms, .. } if *pending_until_ms > unix_ms(now))
    }

    pub(super) fn encode(&self) -> Result<Bytes, NamedLeaseStoreError> {
        Ok(Bytes::from(serde_json::to_vec(self)?))
    }

    pub(super) fn decode(value: &[u8]) -> Result<Self, NamedLeaseStoreError> {
        Ok(serde_json::from_slice(value)?)
    }
}

fn unix_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

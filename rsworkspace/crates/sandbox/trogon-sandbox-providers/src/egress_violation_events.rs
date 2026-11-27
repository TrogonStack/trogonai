use trogon_sandbox_contract::LeaseId;

use crate::{
    EGRESS_PROXY_LEASE_ID_ENV, EGRESS_PROXY_VIOLATION_NATS_URL_ENV, ViolationSinkEndpoint, ViolationSinkEndpointError,
};

/// Where one lease's egress proxy publishes its violation events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressViolationEvents {
    nats_url: String,
    lease_id: LeaseId,
}

impl EgressViolationEvents {
    pub fn new(nats_url: impl Into<String>, lease_id: LeaseId) -> Result<Self, ViolationSinkEndpointError> {
        let nats_url = nats_url.into();
        ViolationSinkEndpoint::parse_all(&nats_url)?;
        Ok(Self { nats_url, lease_id })
    }

    pub fn env_pairs(&self) -> [(&'static str, &str); 2] {
        [
            (EGRESS_PROXY_VIOLATION_NATS_URL_ENV, self.nats_url.as_str()),
            (EGRESS_PROXY_LEASE_ID_ENV, self.lease_id.as_str()),
        ]
    }

    /// Builds the events, or logs and returns `None` when the URL cannot be
    /// parsed. Events are then simply unavailable and egress stays fully
    /// redirected.
    pub fn try_new(nats_url: &str, lease_id: LeaseId) -> Option<Self> {
        match Self::new(nats_url, lease_id) {
            Ok(events) => Some(events),
            Err(error) => {
                tracing::warn!(%error, "egress violation events disabled");
                None
            }
        }
    }
}

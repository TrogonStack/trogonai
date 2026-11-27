use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_sandbox_contract::TenantId;

use super::{LeaseAccess, LeaseLifetime, PauseOwnership};
use crate::LeaseSandbox;
use crate::api::TrafficPrivacy;
use crate::envd::EnvdTarget;

/// A sandbox this worker holds for a lease.
pub(crate) struct ActiveLease {
    pub(crate) access: LeaseAccess,
    pub(crate) tenant: TenantId,
    pub(crate) gate: Arc<Mutex<()>>,
    pub(crate) pause: PauseOwnership,
    pub(crate) sandbox_id: String,
    pub(crate) target: EnvdTarget,
    pub(crate) traffic_token: Option<String>,
    pub(crate) traffic_privacy: TrafficPrivacy,
    pub(crate) domain: String,
    pub(crate) exposed_ports: Vec<u16>,
    pub(crate) lifetime: LeaseLifetime,
    pub(crate) region: Option<String>,
}

impl ActiveLease {
    pub(crate) fn view(&self) -> LeaseSandbox {
        LeaseSandbox {
            sandbox_id: self.sandbox_id.clone(),
            target: self.target.clone(),
            traffic_token: self.traffic_token.clone(),
            domain: self.domain.clone(),
            exposed_ports: self.exposed_ports.clone(),
        }
    }
}

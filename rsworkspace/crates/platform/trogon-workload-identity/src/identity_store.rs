use std::sync::Arc;
use std::time::SystemTime;

use rustls::ServerConfig;
use spiffe::SpiffeId;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{ActivationAudit, ApplicationId, IdentityConfig, RegistrationError, RegistrationSet};

#[derive(Clone)]
pub struct IdentityStore {
    config: IdentityConfig,
}

pub struct IdentityDriver;

pub struct MaterialLease {
    tls: Arc<ServerConfig>,
    changed: Arc<Notify>,
}

impl IdentityStore {
    pub fn new(config: IdentityConfig, _audit: Arc<dyn ActivationAudit>) -> (Self, IdentityDriver) {
        (Self { config }, IdentityDriver)
    }
    pub fn config(&self) -> &IdentityConfig { &self.config }
    pub fn replace_registration(&self, _registration: RegistrationSet) -> Result<(), RegistrationError> { Ok(()) }
    pub fn withdraw_registration(&self) {}
    pub fn material(&self) -> Option<MaterialLease> { None }
    pub fn source_revision(&self) -> u64 { 0 }
}

impl IdentityDriver {
    pub async fn run(self, shutdown: CancellationToken) { shutdown.cancelled().await; }
}

impl MaterialLease {
    pub fn server_config(&self) -> Arc<ServerConfig> { Arc::clone(&self.tls) }
    pub fn is_current(&self) -> bool { false }
    pub async fn changed(&self) { self.changed.notified().await; }
    pub fn deadline(&self) -> Instant { Instant::now() }
    pub fn expires_at(&self) -> SystemTime { SystemTime::now() }
    pub fn application_for_peer(&self, _peer: &SpiffeId) -> Option<ApplicationId> { None }
}

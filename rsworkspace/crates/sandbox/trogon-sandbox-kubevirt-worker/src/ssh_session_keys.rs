use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, warn};
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_ssh_gateway::{
    SshAttachRequest, SshAttachResponse, SshDetachRequest, SshDetachResponse, SshSessionId,
};

use crate::{GuestAccess, KubeVirtGuestAccess};

/// The SSH session keys installed in the guests of the leases this worker holds, so a direct `sshd` login with a
/// session's key keeps working alongside the stream bridge.
pub struct SshSessionKeys {
    access: Arc<KubeVirtGuestAccess>,
    keys: Mutex<HashMap<LeaseId, HashMap<SshSessionId, String>>>,
}

impl SshSessionKeys {
    /// Installs and removes keys through `access`.
    pub fn new(access: Arc<KubeVirtGuestAccess>) -> Self {
        Self {
            access,
            keys: Mutex::new(HashMap::new()),
        }
    }

    /// Installs the session's key in the lease's guest and remembers it, while the lease is still held, for
    /// [`Self::detach`].
    pub async fn attach(&self, request: &SshAttachRequest) -> SshAttachResponse {
        let lease_id = &request.lease_id;
        if let Err(error) = self.access.install_key(lease_id, &request.public_key).await {
            warn!(%error, lease_id = %lease_id, "failed to attach ssh key");
            return SshAttachResponse::rejected(error.to_string());
        }
        if self.access.holds(lease_id).await {
            let mut keys = self.keys.lock().await;
            let sessions = keys.entry(lease_id.clone()).or_default();
            sessions.insert(request.session_id.clone(), request.public_key.clone());
        }
        debug!(lease_id = %lease_id, session_id = %request.session_id, "ssh key attached");
        SshAttachResponse::accepted()
    }

    /// Removes the session's key from the lease's guest. A session with no key on record is already detached.
    pub async fn detach(&self, request: &SshDetachRequest) -> SshDetachResponse {
        let lease_id = &request.lease_id;
        if !self.access.holds(lease_id).await {
            return SshDetachResponse::rejected(format!("unknown lease {lease_id}"));
        }
        let Some(public_key) = self.take(lease_id, &request.session_id).await else {
            return SshDetachResponse {
                ok: true,
                message: Some("session not found".to_string()),
            };
        };
        if let Err(error) = self.access.remove_key(lease_id, &public_key).await {
            warn!(%error, lease_id = %lease_id, "failed to detach ssh key");
            return SshDetachResponse::rejected(error.to_string());
        }
        SshDetachResponse::accepted()
    }

    /// Forgets every key on record for a released lease.
    pub async fn forget_lease(&self, lease_id: &LeaseId) {
        self.keys.lock().await.remove(lease_id);
    }

    /// Session keys on record for `lease_id`.
    pub async fn key_count(&self, lease_id: &LeaseId) -> usize {
        self.keys.lock().await.get(lease_id).map_or(0, HashMap::len)
    }

    async fn take(&self, lease_id: &LeaseId, session_id: &SshSessionId) -> Option<String> {
        let mut keys = self.keys.lock().await;
        let sessions = keys.get_mut(lease_id)?;
        let public_key = sessions.remove(session_id);
        if sessions.is_empty() {
            keys.remove(lease_id);
        }
        public_key
    }
}

use std::net::SocketAddr;

use async_trait::async_trait;
use trogon_sandbox_contract::LeaseId;

use crate::GuestAccessError;

/// What the SSH client needs to know about a guest. [`crate::KubeVirtGuestAccess`] implements it for KubeVirt.
#[async_trait]
pub trait GuestAccess: Send + Sync {
    /// Whether this worker holds `lease_id`.
    async fn holds(&self, lease_id: &LeaseId) -> bool;

    /// Where the guest's `sshd` listens.
    async fn address(&self, lease_id: &LeaseId) -> Result<SocketAddr, GuestAccessError>;

    /// Adds `public_key` (an `authorized_keys` line) for [`crate::GUEST_USER`].
    async fn install_key(&self, lease_id: &LeaseId, public_key: &str) -> Result<(), GuestAccessError>;
}

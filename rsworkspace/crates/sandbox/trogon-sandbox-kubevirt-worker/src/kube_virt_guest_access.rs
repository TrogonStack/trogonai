use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::LeaseId;

use crate::constants::GUEST_SSH_PORT;
use crate::{GuestAccess, GuestAccessError, GuestExec, GuestLocator};

/// [`GuestAccess`] over the VMI status and the guest exec path.
pub struct KubeVirtGuestAccess {
    locator: Arc<dyn GuestLocator>,
    exec: GuestExec,
    namespace: String,
}

impl KubeVirtGuestAccess {
    /// Reaches the guests `locator` knows of in `namespace`, installing keys through `exec`.
    pub fn new(locator: Arc<dyn GuestLocator>, exec: GuestExec, namespace: impl Into<String>) -> Self {
        Self {
            locator,
            exec,
            namespace: namespace.into(),
        }
    }

    /// Removes `public_key` from [`crate::GUEST_USER`]'s `authorized_keys`.
    pub async fn remove_key(&self, lease_id: &LeaseId, public_key: &str) -> Result<(), GuestAccessError> {
        let vm_name = self.vm_name(lease_id).await?;
        let removed = self
            .exec
            .remove_authorized_key(&self.namespace, &vm_name, public_key)
            .await;
        removed.map_err(GuestAccessError::Exec)
    }

    async fn vm_name(&self, lease_id: &LeaseId) -> Result<String, GuestAccessError> {
        let vm_name = self.locator.vm_name(lease_id).await;
        vm_name.ok_or_else(|| GuestAccessError::UnknownLease {
            lease_id: lease_id.clone(),
        })
    }
}

#[async_trait]
impl GuestAccess for KubeVirtGuestAccess {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.locator.vm_name(lease_id).await.is_some()
    }

    async fn address(&self, lease_id: &LeaseId) -> Result<SocketAddr, GuestAccessError> {
        let vm_name = self.vm_name(lease_id).await?;
        let address = match self.locator.vmi_ip(&vm_name).await {
            Ok(address) => address,
            Err(source) => return Err(GuestAccessError::Address { vm_name, source }),
        };
        match address.parse::<IpAddr>() {
            Ok(ip) => Ok(SocketAddr::new(ip, GUEST_SSH_PORT)),
            Err(source) => Err(GuestAccessError::UnusableAddress {
                vm_name,
                address,
                source,
            }),
        }
    }

    async fn install_key(&self, lease_id: &LeaseId, public_key: &str) -> Result<(), GuestAccessError> {
        let vm_name = self.vm_name(lease_id).await?;
        let installed = self
            .exec
            .append_authorized_key(&self.namespace, &vm_name, public_key)
            .await;
        installed.map_err(GuestAccessError::Exec)
    }
}

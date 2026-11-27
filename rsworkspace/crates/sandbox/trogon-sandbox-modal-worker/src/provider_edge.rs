use std::time::{SystemTime, UNIX_EPOCH};

use trogon_sandbox_contract::{
    GetSandboxDomainResponse, LeaseError, LeaseId, NativeSnapshotReceipt, NativeSnapshotRef, PortAccess, ProviderKind,
    SnapshotExportError, SnapshotExportOutcome,
};

use crate::constants::NATIVE_SNAPSHOT_TTL;
use crate::{ModalApiError, ModalProvider, ModalProviderError};

impl ModalProvider {
    /// Resolves a declared port to its Modal tunnel. `None` means this worker does not hold the lease, so another
    /// replica should answer.
    pub async fn get_domain(&self, lease_id: &LeaseId, port: u16) -> Option<GetSandboxDomainResponse> {
        let sandbox_id = {
            let active = self.active.lock().await;
            let held = active.get(lease_id)?;
            if !held.exposed_ports.iter().any(|exposed| exposed.get() == port) {
                return Some(GetSandboxDomainResponse::PortNotExposed);
            }
            held.sandbox_id.clone()
        };
        let tunnels = match self.api.tunnels(&sandbox_id).await {
            Ok(tunnels) => tunnels,
            Err(error) => {
                return Some(GetSandboxDomainResponse::Denied(LeaseError::Backend {
                    message: error.to_string(),
                }));
            }
        };
        Some(match tunnels.into_iter().find(|tunnel| tunnel.port == port) {
            Some(tunnel) => GetSandboxDomainResponse::Available {
                endpoint: format!("{}:{}", tunnel.host, tunnel.tls_port),
                url: None,
                expires_at: None,
                access: Some(PortAccess {
                    base_url: tunnel.url,
                    headers: Default::default(),
                }),
            },
            None => GetSandboxDomainResponse::Denied(LeaseError::Backend {
                message: format!("Modal reported no tunnel for declared port {port}"),
            }),
        })
    }

    /// Snapshots the filesystem of the sandbox behind `lease_id`. `None` means this worker does not hold the
    /// lease, so another replica should answer.
    pub async fn native_snapshot(&self, lease_id: &LeaseId) -> Option<SnapshotExportOutcome> {
        let sandbox_id = self.active.lock().await.get(lease_id)?.sandbox_id.clone();
        Some(
            match self
                .api
                .snapshot_filesystem(&sandbox_id, NATIVE_SNAPSHOT_TTL.as_secs())
                .await
            {
                Ok(id) => SnapshotExportOutcome::Native(NativeSnapshotReceipt {
                    reference: NativeSnapshotRef {
                        provider: ProviderKind::Modal,
                        id,
                        expires_at: Some(unix_now() + NATIVE_SNAPSHOT_TTL.as_secs()),
                    },
                    bytes: None,
                }),
                Err(error) => SnapshotExportOutcome::Err(SnapshotExportError::Transient {
                    message: format!("Modal snapshot_filesystem: {error}"),
                }),
            },
        )
    }

    /// Ends the lease: a read-write volume is written back, then the sandbox is terminated. A lease this worker
    /// does not hold is already ended.
    pub async fn cancel_inner(&self, lease_id: &LeaseId) -> Result<(), ModalProviderError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let held = self
            .active
            .lock()
            .await
            .get(lease_id)
            .map(|held| (held.sandbox_id.clone(), held.workspace.clone(), held.tenant.clone()));
        let Some((sandbox_id, workspace, tenant)) = held else {
            return Ok(());
        };
        workspace.push_changes(&tenant, self.stores.as_ref()).await?;
        match self.api.terminate(&sandbox_id).await {
            Ok(()) | Err(ModalApiError::SandboxNotFound(_)) => {
                self.active.lock().await.remove(lease_id);
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

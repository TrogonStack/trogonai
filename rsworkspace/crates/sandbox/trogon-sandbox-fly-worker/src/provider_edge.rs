use std::collections::BTreeMap;

use trogon_sandbox_contract::{GetSandboxDomainResponse, LeaseError, LeaseId, PortAccess};

use crate::{FlyPreviewNetwork, FlyProvider, FlyProviderError};

/// How a failed lease operation is reported to the caller.
pub(crate) fn denial(lease: &LeaseId, error: &FlyProviderError) -> LeaseError {
    match error {
        FlyProviderError::NotHeld => LeaseError::LeaseNotFound {
            lease_id: lease.clone(),
        },
        other => LeaseError::Backend {
            message: other.to_string(),
        },
    }
}

impl FlyProvider {
    fn public_host(&self) -> (String, &'static str) {
        let app = self.api.app();
        match self.config.preview_network {
            FlyPreviewNetwork::Public => (
                self.config
                    .public_host
                    .clone()
                    .unwrap_or_else(|| format!("{app}.fly.dev")),
                "https",
            ),
            FlyPreviewNetwork::Flycast => (format!("{app}.flycast"), "http"),
        }
    }

    /// Where an exposed port of the lease can be reached.
    pub async fn get_domain(&self, lease: &LeaseId, port: u16) -> GetSandboxDomainResponse {
        let record = match self.record(lease).await {
            Ok(record) => record,
            Err(error) => return GetSandboxDomainResponse::Denied(denial(lease, &error)),
        };
        if !record.exposed_ports.iter().any(|exposed| exposed.get() == port) {
            return GetSandboxDomainResponse::PortNotExposed;
        }
        let (host, scheme) = self.public_host();
        GetSandboxDomainResponse::Available {
            endpoint: format!("{host}:{port}"),
            url: None,
            expires_at: None,
            access: Some(PortAccess {
                base_url: format!("{scheme}://{host}:{port}"),
                headers: BTreeMap::from([("fly-force-instance-id".to_owned(), record.machine.as_str().to_owned())]),
            }),
        }
    }

    /// Suspends the lease's Machine, memory included.
    pub async fn pause(&self, lease: &LeaseId) -> Result<(), LeaseError> {
        let record = self.record(lease).await.map_err(|error| denial(lease, &error))?;
        self.suspend(&record.machine)
            .await
            .map_err(|error| denial(lease, &error))
    }

    /// Starts a suspended Machine again.
    pub async fn thaw(&self, lease: &LeaseId) -> Result<(), LeaseError> {
        let record = self.record(lease).await.map_err(|error| denial(lease, &error))?;
        self.resume(&record.machine)
            .await
            .map_err(|error| denial(lease, &error))
    }

    /// Ends the lease: its Machine and workspace volume are destroyed. A lease this worker does not hold is
    /// already ended.
    pub async fn release(&self, lease: &LeaseId) -> Result<(), FlyProviderError> {
        let _guard = self.allocation.lock().await;
        let Ok(record) = self.record(lease).await else {
            return Ok(());
        };
        self.destroy_owned(&record.machine).await?;
        if let Some(volume) = &record.workspace_volume {
            self.delete_workspace_volume(volume).await?;
        }
        self.forget(lease).await;
        Ok(())
    }
}

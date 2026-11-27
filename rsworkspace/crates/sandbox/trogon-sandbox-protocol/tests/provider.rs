//! Default behavior of the `Provider` trait and its error type.

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::json;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, OciArtifactRef, ProviderKind, ProviderLease, SnapshotReceipt};
use trogon_sandbox_protocol::{Provider, ProviderError, SnapshotError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Recording {
    cancelled: Mutex<Vec<String>>,
}

#[async_trait]
impl Provider for Recording {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Mock
    }

    async fn lease(&self, _req: &LeaseRequest, _lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        Err(ProviderError::backend("no leases here"))
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.cancelled
            .lock()
            .map_err(|_| ProviderError::backend("poisoned"))?
            .push(lease_id.as_str().to_string());
        Ok(())
    }
}

fn target() -> Result<OciArtifactRef, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(json!({
        "registry": "ghcr.io",
        "repository": "acme/snap",
        "digest": format!("sha256:{}", "00".repeat(32)),
        "artifact_type": "sandbox_rootfs_v1",
    }))?)
}

#[tokio::test]
async fn purge_defaults_to_cancel() -> TestResult {
    let provider = Recording::default();
    provider.purge(&LeaseId::new("l-1")).await?;
    let cancelled = provider.cancelled.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(cancelled, vec!["l-1".to_string()]);
    assert_eq!(provider.kind(), ProviderKind::Mock);
    Ok(())
}

#[tokio::test]
async fn snapshot_is_unsupported_by_default() -> TestResult {
    let provider = Recording::default();
    let error: ProviderError = provider
        .snapshot(&LeaseId::new("l-1"), &target()?)
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, ProviderError::Snapshot(SnapshotError::Unsupported)));
    assert_eq!(error.to_string(), "snapshot not supported by this provider");
    Ok(())
}

#[tokio::test]
async fn checkpoint_running_delegates_to_snapshot() -> TestResult {
    let provider = Recording::default();
    let result: Result<SnapshotReceipt, ProviderError> =
        provider.checkpoint_running(&LeaseId::new("l-1"), &target()?).await;
    assert!(matches!(
        result,
        Err(ProviderError::Snapshot(SnapshotError::Unsupported))
    ));
    Ok(())
}

#[tokio::test]
async fn lease_failures_carry_the_backend_message() -> TestResult {
    let provider = Recording::default();
    let request: LeaseRequest = serde_json::from_value(json!({
        "tenant": "acme",
        "provider_pref": ["mock"],
        "spec": {
            "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
            "workspace": { "empty": {} },
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
            "privileged": false,
            "boot_mode": "entrypoint_default",
        },
        "ttl_seconds": 60,
        "idempotency_key": "key-1",
    }))?;
    let error = provider
        .lease(&request, &LeaseId::new("l-1"))
        .await
        .err()
        .ok_or("expected an error")?;
    assert_eq!(error.to_string(), "no leases here");
    Ok(())
}

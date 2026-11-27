//! A fake hypervisor, an in-memory registry and the builders the tests share.

#![allow(dead_code)]

mod hypervisor;
mod registry;

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tempfile::TempDir;
use trogon_sandbox_contract::{ArtifactType, Digest, LeaseId, LeaseRequest, OciArtifactRef, ProviderDispatchPayload};
use trogon_sandbox_hyperlight_worker::{HyperlightRuntime, HyperlightWorkerProvider};
use trogon_sandbox_oci::OciClient;

pub use hypervisor::FakeHypervisor;
pub use registry::{FakeRegistry, sha256};

pub type TestResult = Result<(), Box<dyn Error>>;

pub fn lease(id: &str) -> LeaseId {
    LeaseId::new(id)
}

/// A lease request for the component `name`, with the pieces a test varies given as JSON.
pub fn request_with(image: Value, workspace: Value, extra: Value) -> Result<LeaseRequest, Box<dyn Error>> {
    let mut spec = json!({
        "image": image,
        "workspace": workspace,
        "cpu_seconds": 1,
        "memory_mib": 64,
    });
    if let (Some(spec), Some(extra)) = (spec.as_object_mut(), extra.as_object()) {
        spec.extend(extra.clone());
    }
    let request = json!({
        "tenant": "acme",
        "provider_pref": ["hyperlight"],
        "spec": spec,
        "ttl_seconds": 60,
        "idempotency_key": "k",
    });
    Ok(serde_json::from_value(request)?)
}

pub fn component_image(name: &str) -> Value {
    json!({"kind": "registry", "reference": {"registry": "", "repository": name}})
}

pub fn empty_workspace() -> Value {
    json!({"empty": {}})
}

/// A request for the component `name` with an empty workspace.
pub fn request(name: &str) -> Result<LeaseRequest, Box<dyn Error>> {
    request_with(component_image(name), empty_workspace(), json!({}))
}

pub fn dispatch(id: &str, name: &str) -> Result<Value, Box<dyn Error>> {
    let payload = ProviderDispatchPayload {
        lease_id: lease(id),
        request: request(name)?,
        correlation_id: None,
        resolved_drives: Vec::new(),
        target: None,
    };
    Ok(serde_json::to_value(payload)?)
}

pub fn artifact_ref(
    registry: &str,
    repository: &str,
    digest: &str,
    artifact_type: ArtifactType,
) -> Result<OciArtifactRef, Box<dyn Error>> {
    Ok(OciArtifactRef {
        registry: registry.to_owned(),
        repository: repository.to_owned(),
        digest: Digest::new(digest)?,
        artifact_type,
    })
}

/// A provider over a fake hypervisor, a registry and a component root holding `echo.wasm`.
pub struct Rig {
    pub hypervisor: FakeHypervisor,
    pub registry: FakeRegistry,
    pub provider: Arc<HyperlightWorkerProvider<FakeHypervisor>>,
    pub root: PathBuf,
    pub dir: TempDir,
}

impl Rig {
    pub async fn start() -> Result<Self, Box<dyn Error>> {
        Self::with(4, 8).await
    }

    pub async fn with(max_warm: u32, max_leases: u32) -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("components");
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("echo.wasm"), "echo")?;
        let hypervisor = FakeHypervisor::default();
        let registry = FakeRegistry::start().await?;
        let client: Arc<OciClient> = Arc::new(registry.client()?);
        let address = registry.address.clone();
        let runtime = HyperlightRuntime::new(hypervisor.clone(), root.clone(), max_warm, client, address)?;
        let provider = Arc::new(HyperlightWorkerProvider::new(runtime, max_leases));
        Ok(Self {
            hypervisor,
            registry,
            provider,
            root,
            dir,
        })
    }

    pub fn target(&self, repository: &str) -> Result<OciArtifactRef, Box<dyn Error>> {
        artifact_ref(
            &self.registry.address,
            repository,
            &sha256(b"placeholder"),
            ArtifactType::HyperlightSnapshotV1,
        )
    }
}

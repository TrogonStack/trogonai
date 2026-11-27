use crate::snapshot_catalog::ArtifactLister;

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{ArtifactType, Digest, OciArtifactRef, TenantId};

use std::collections::HashMap;

use trogon_sandbox_contract::{LeaseId, SnapshotKvRecord};

use super::*;
use crate::snapshot_catalog::InMemorySnapshotCatalog;

fn digest(c: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", c.to_string().repeat(64)))?)
}
fn staging_tag(c: char) -> String {
    format!("sandbox-artifact-sha256-{}", c.to_string().repeat(64))
}

#[derive(Default)]
struct FakeInventory {
    tags: HashMap<String, Vec<String>>,
    types: HashMap<Digest, ArtifactType>,
    failing: HashSet<String>,
}

#[async_trait]
impl SnapshotRegistryInventory for FakeInventory {
    async fn tags(&self, _registry: &str, repository: &str) -> Result<Vec<String>, RegistryInventoryError> {
        if self.failing.contains(repository) {
            return Err(trogon_sandbox_oci::OciError::RegistryRejected("registry down".to_owned()).into());
        }
        Ok(self.tags.get(repository).cloned().unwrap_or_default())
    }

    async fn artifact_type(&self, artifact: &OciArtifactRef) -> Option<ArtifactType> {
        self.types.get(&artifact.digest).copied()
    }
}

fn record(tenant: &str, digest: Digest) -> Result<SnapshotKvRecord, Box<dyn std::error::Error>> {
    Ok(SnapshotKvRecord {
        lease_id: LeaseId::new("l"),
        tenant: TenantId::new(tenant)?,
        digest,
        artifact_type: ArtifactType::SandboxRootfsV1,
        registry: "registry.local".into(),
        repository: format!("snapshots/{tenant}"),
        parent: None,
        created_at: None,
        env_digest: None,
    })
}

async fn lister(
    inventory: FakeInventory,
    records: Vec<SnapshotKvRecord>,
    configured: Vec<TenantId>,
) -> OciArtifactLister {
    let catalog = Arc::new(InMemorySnapshotCatalog::default());
    for r in records {
        catalog.insert(r).await;
    }
    OciArtifactLister::new(Arc::new(inventory), catalog, "registry.local".into(), configured)
}

#[tokio::test]
async fn staging_tags_become_listed_snapshot_artifacts() -> Result<(), Box<dyn std::error::Error>> {
    let mut inventory = FakeInventory::default();
    inventory.tags.insert(
        "snapshots/acme".into(),
        vec![staging_tag('a'), "latest".into(), staging_tag('b')],
    );
    inventory.types.insert(digest('a')?, ArtifactType::SandboxRootfsV1);
    inventory
        .types
        .insert(digest('b')?, ArtifactType::FirecrackerSnapshotV1);
    let listed = lister(inventory, vec![record("acme", digest('a')?)?], vec![])
        .await
        .list()
        .await?;
    let found: Vec<_> = listed.iter().map(|a| a.reference.digest.clone()).collect();
    assert_eq!(found, vec![digest('a')?, digest('b')?]);
    assert!(listed.iter().all(|a| a.tenant.as_str() == "acme"));
    assert_eq!(listed[1].reference.artifact_type, ArtifactType::FirecrackerSnapshotV1);
    Ok(())
}
#[tokio::test]
async fn referrers_and_unreadable_manifests_are_not_listed() -> Result<(), Box<dyn std::error::Error>> {
    let mut inventory = FakeInventory::default();
    inventory.tags.insert(
        "snapshots/acme".into(),
        vec![staging_tag('a'), staging_tag('b'), staging_tag('c')],
    );
    inventory.types.insert(digest('a')?, ArtifactType::SbomV1);
    inventory.types.insert(digest('b')?, ArtifactType::ScanReportV1);
    let listed = lister(inventory, vec![], vec![TenantId::new("acme")?])
        .await
        .list()
        .await?;
    assert!(listed.is_empty());
    Ok(())
}
#[tokio::test]
async fn a_configured_tenant_with_no_records_is_still_listed() -> Result<(), Box<dyn std::error::Error>> {
    let mut inventory = FakeInventory::default();
    inventory.tags.insert("snapshots/ghost".into(), vec![staging_tag('d')]);
    inventory.types.insert(digest('d')?, ArtifactType::SandboxRootfsV1);
    let listed = lister(inventory, vec![], vec![TenantId::new("ghost")?])
        .await
        .list()
        .await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].tenant.as_str(), "ghost");
    assert_eq!(listed[0].reference.repository, "snapshots/ghost");
    Ok(())
}
#[tokio::test]
async fn a_failing_repository_is_skipped_and_the_rest_listed() -> Result<(), Box<dyn std::error::Error>> {
    let mut inventory = FakeInventory::default();
    inventory.failing.insert("snapshots/down".into());
    inventory.tags.insert("snapshots/up".into(), vec![staging_tag('e')]);
    inventory.types.insert(digest('e')?, ArtifactType::SandboxRootfsV1);
    let listed = lister(inventory, vec![], vec![TenantId::new("down")?, TenantId::new("up")?])
        .await
        .list()
        .await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].tenant.as_str(), "up");
    Ok(())
}

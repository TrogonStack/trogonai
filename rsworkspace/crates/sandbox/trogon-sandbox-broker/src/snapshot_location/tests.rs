use trogon_sandbox_contract::{ArtifactType, Digest, OciArtifactRef, TenantId};

use async_trait::async_trait;

use std::collections::HashSet;
use std::sync::Mutex;

use super::snapshot_manifest_probe::SnapshotManifestProbe;
use super::*;

struct FakeRegistry {
    repositories: HashSet<String>,
    probed: Mutex<Vec<String>>,
}

impl FakeRegistry {
    fn holding(repositories: &[&str]) -> Self {
        Self {
            repositories: repositories.iter().map(|r| r.to_string()).collect(),
            probed: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl SnapshotManifestProbe for FakeRegistry {
    async fn exists(&self, artifact: &OciArtifactRef) -> bool {
        self.probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(artifact.repository.clone());
        self.repositories.contains(&artifact.repository)
    }
}

fn digest() -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )?)
}

async fn locate(registry: &FakeRegistry, tenant: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(locate_snapshot_repository(
        Some(registry),
        "registry.local",
        &TenantId::new(tenant)?,
        &digest()?,
        ArtifactType::SandboxRootfsV1,
    )
    .await)
}

#[tokio::test]
async fn legacy_snapshot_is_found_until_a_current_copy_exists() -> Result<(), Box<dyn std::error::Error>> {
    let legacy_only = FakeRegistry::holding(&["snapshots/a_b"]);
    assert_eq!(locate(&legacy_only, "a:b").await?, "snapshots/a_b");

    let both = FakeRegistry::holding(&["snapshots/a_b", "snapshots/tenant-hex/613a62"]);
    assert_eq!(locate(&both, "a:b").await?, "snapshots/tenant-hex/613a62");

    let neither = FakeRegistry::holding(&[]);
    assert_eq!(locate(&neither, "a:b").await?, "snapshots/tenant-hex/613a62");
    Ok(())
}

#[tokio::test]
async fn tenants_with_one_repository_are_never_probed() -> Result<(), Box<dyn std::error::Error>> {
    let registry = FakeRegistry::holding(&[]);
    assert_eq!(locate(&registry, "a_b").await?, "snapshots/a_b");
    assert_eq!(locate(&registry, "a.b").await?, "snapshots/a.b");
    assert!(
        registry
            .probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}

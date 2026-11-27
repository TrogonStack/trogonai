use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;
use trogon_sandbox_contract::{ArtifactType, Digest, OciArtifactRef, TenantId};
use trogon_sandbox_protocol::{legacy_snapshot_repository_for_tenant, snapshot_repository_for_tenant};

use super::snapshot_registry_inventory::SnapshotRegistryInventory;
use crate::snapshot_catalog::{ArtifactListError, ArtifactLister, ListedArtifact, SnapshotCatalog};

fn is_snapshot_type(artifact_type: ArtifactType) -> bool {
    matches!(
        artifact_type,
        ArtifactType::SandboxRootfsV1
            | ArtifactType::HyperlightSnapshotV1
            | ArtifactType::FirecrackerSnapshotV1
            | ArtifactType::KubevirtDiskV1
    )
}

pub struct OciArtifactLister {
    inventory: Arc<dyn SnapshotRegistryInventory>,
    catalog: Arc<dyn SnapshotCatalog>,
    registry: String,
    tenants: Vec<TenantId>,
}

impl OciArtifactLister {
    pub fn new(
        inventory: Arc<dyn SnapshotRegistryInventory>,
        catalog: Arc<dyn SnapshotCatalog>,
        registry: String,
        tenants: Vec<TenantId>,
    ) -> Self {
        Self {
            inventory,
            catalog,
            registry,
            tenants,
        }
    }
}

#[async_trait]
impl ArtifactLister for OciArtifactLister {
    async fn list(&self) -> Result<Vec<ListedArtifact>, ArtifactListError> {
        let records = self.catalog.all().await?;

        // (registry, repository) -> tenant that owns the repository.
        let mut repositories: BTreeSet<(String, String, TenantId)> = BTreeSet::new();
        let mut tenants: Vec<TenantId> = self.tenants.clone();
        for record in &records {
            repositories.insert((
                record.registry.clone(),
                record.repository.clone(),
                record.tenant.clone(),
            ));
            tenants.push(record.tenant.clone());
        }
        for tenant in &tenants {
            for repository in [
                snapshot_repository_for_tenant(tenant),
                legacy_snapshot_repository_for_tenant(tenant),
            ] {
                repositories.insert((self.registry.clone(), repository, tenant.clone()));
            }
        }

        let mut seen: HashSet<(String, String, Digest)> = HashSet::new();
        let mut out = Vec::new();
        for (registry, repository, tenant) in repositories {
            let tags = match self.inventory.tags(&registry, &repository).await {
                Ok(tags) => tags,
                Err(e) => {
                    warn!(
                        %registry, %repository, error = %e,
                        "artifact lister: failed to list tags; skipping the repository"
                    );
                    continue;
                }
            };
            for digest in tags
                .iter()
                .filter_map(|tag| trogon_sandbox_oci::digest_from_staging_tag(tag))
            {
                if !seen.insert((registry.clone(), repository.clone(), digest.clone())) {
                    continue;
                }
                let probe = OciArtifactRef {
                    registry: registry.clone(),
                    repository: repository.clone(),
                    digest,
                    artifact_type: ArtifactType::SandboxRootfsV1,
                };
                let Some(artifact_type) = self.inventory.artifact_type(&probe).await else {
                    continue;
                };
                if !is_snapshot_type(artifact_type) {
                    continue;
                }
                out.push(ListedArtifact {
                    tenant: tenant.clone(),
                    reference: OciArtifactRef { artifact_type, ..probe },
                });
            }
        }
        Ok(out)
    }
}

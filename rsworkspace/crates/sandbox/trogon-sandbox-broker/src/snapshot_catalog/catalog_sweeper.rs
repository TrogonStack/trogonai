use std::collections::HashSet;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use tokio::sync::Mutex;
use tracing::{info, warn};
use trogon_sandbox_contract::{Digest, SnapshotDeleted, SnapshotKvRecord, TenantId};
use trogon_sandbox_protocol::subjects::SNAPSHOT_DELETED;

use super::artifact_deleter::ArtifactDeleter;
use super::artifact_lister::ArtifactLister;
use super::catalog::SnapshotCatalog;
use super::catalog_sweep_config::CatalogSweepConfig;
use super::catalog_sweep_report::CatalogSweepReport;
use super::orphan_tracker::OrphanTracker;
use super::retention_planning::{close_over_parents, find_orphans, plan_keep_n};
use crate::bus::BrokerBus;
use crate::snapshot_tracker::SnapshotRemovalError;

pub struct CatalogSweeper {
    catalog: Arc<dyn SnapshotCatalog>,
    lister: Option<Arc<dyn ArtifactLister>>,
    deleter: Arc<dyn ArtifactDeleter>,
    bus: Arc<dyn BrokerBus>,
    config: CatalogSweepConfig,
    tracker: Mutex<OrphanTracker>,
    accounting: Option<Arc<crate::snapshot_accounting::SnapshotAccountingStore>>,
    live: Option<Arc<dyn crate::snapshot_tracker::LiveSnapshotStore>>,
}

impl CatalogSweeper {
    pub fn new(
        catalog: Arc<dyn SnapshotCatalog>,
        lister: Option<Arc<dyn ArtifactLister>>,
        deleter: Arc<dyn ArtifactDeleter>,
        bus: Arc<dyn BrokerBus>,
        config: CatalogSweepConfig,
    ) -> Self {
        Self {
            catalog,
            lister,
            deleter,
            bus,
            config,
            tracker: Mutex::new(OrphanTracker::default()),
            accounting: None,
            live: None,
        }
    }

    pub fn with_accounting(
        mut self,
        accounting: Arc<crate::snapshot_accounting::SnapshotAccountingStore>,
        live: Arc<dyn crate::snapshot_tracker::LiveSnapshotStore>,
    ) -> Self {
        self.accounting = Some(accounting);
        self.live = Some(live);
        self
    }

    async fn remove_owned(
        &self,
        tenant: &TenantId,
        digest: &Digest,
    ) -> Result<Option<crate::snapshot_accounting::SnapshotDeletionClaim>, SnapshotRemovalError> {
        let accounting = self
            .accounting
            .as_ref()
            .ok_or(SnapshotRemovalError::AccountingUnavailable)?;
        let live = self.live.as_ref().ok_or(SnapshotRemovalError::LiveUnavailable)?;
        crate::snapshot_tracker::remove_accounted_snapshot(
            accounting,
            live.as_ref(),
            self.deleter.as_ref(),
            tenant,
            digest,
        )
        .await
    }

    /// One pass. `restore_sources` and `tag_pinned` come from the caller, who
    /// owns the lease and tag stores; the ancestors of both are added here.
    pub async fn sweep_once(
        &self,
        restore_sources: HashSet<Digest>,
        tag_pinned: HashSet<Digest>,
        now: SystemTime,
    ) -> CatalogSweepReport {
        let mut report = CatalogSweepReport::default();
        let records = match self.catalog.all().await {
            Ok(records) => records,
            Err(e) => {
                warn!(error = %e, "snapshot catalog sweep: failed to read the catalog");
                return report;
            }
        };
        let mut roots = restore_sources;
        roots.extend(tag_pinned);
        let protected = close_over_parents(roots, &records);

        for record in plan_keep_n(&records, self.config.keep_per_lease, &protected) {
            if self.prune(&record).await {
                report.pruned += 1;
            }
        }

        if let Some(lister) = &self.lister {
            match lister.list().await {
                Ok(listed) => {
                    let orphans = find_orphans(&listed, &records, &protected);
                    let ready = self
                        .tracker
                        .lock()
                        .await
                        .observe(&orphans, now, self.config.orphan_grace);
                    for artifact in ready {
                        match self.remove_owned(&artifact.tenant, &artifact.reference.digest).await {
                            Ok(Some(claim)) => {
                                report.orphans_deleted += 1;
                                self.publish_deleted(&claim).await;
                                info!(
                                    tenant = %artifact.tenant.as_str(),
                                    digest = %artifact.reference.digest,
                                    "snapshot catalog sweep: deleted orphan artifact"
                                );
                            }
                            Ok(None) => {}
                            Err(e) => warn!(
                                tenant = %artifact.tenant.as_str(),
                                digest = %artifact.reference.digest,
                                error = %e,
                                "snapshot catalog sweep: failed to delete orphan artifact"
                            ),
                        }
                    }
                }
                Err(e) => warn!(error = %e, "snapshot catalog sweep: failed to list artifacts"),
            }
        }
        report
    }

    /// Deletes the artifact, then its record. The record stays when the
    /// registry delete fails, so the next pass tries again.
    async fn prune(&self, record: &SnapshotKvRecord) -> bool {
        let claim = match self.remove_owned(&record.tenant, &record.digest).await {
            Ok(Some(claim)) => claim,
            Ok(None) => return false,
            Err(error) => {
                warn!(tenant = %record.tenant.as_str(), digest = %record.digest, %error, "snapshot catalog deletion remains pending");
                return false;
            }
        };
        if let Err(e) = self.catalog.remove(record).await {
            warn!(
                tenant = %record.tenant.as_str(),
                digest = %record.digest,
                error = %e,
                "snapshot catalog sweep: artifact deleted but its record was not removed"
            );
        }
        self.publish_deleted(&claim).await;
        true
    }

    async fn publish_deleted(&self, claim: &crate::snapshot_accounting::SnapshotDeletionClaim) {
        let tenant = &claim.tenant;
        let digest = &claim.reference.digest;
        let event = SnapshotDeleted {
            digest: digest.clone(),
            tenant: tenant.clone(),
            bytes: claim.entry.bytes,
            generation: Some(claim.generation.clone()),
        };
        let Ok(payload) = serde_json::to_vec(&event) else {
            return;
        };
        if let Err(e) = self.bus.publish(SNAPSHOT_DELETED, Bytes::from(payload)).await {
            warn!(tenant = %tenant.as_str(), digest = %digest, error = %e, "snapshot catalog sweep: failed to publish snapshot_deleted");
        }
    }
}

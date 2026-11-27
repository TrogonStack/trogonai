use std::sync::Arc;
use std::time::SystemTime;

use async_nats::Client;
use tracing::{Instrument, info};
use trogon_sandbox_broker::snapshot_accounting::SnapshotAccountingStore;
use trogon_sandbox_broker::snapshot_tag::SnapshotTagStore;
use trogon_sandbox_contract::{
    LeaseReleased, OciArtifactRef, SnapshotAccountingDisposition, SnapshotKvRecord, SnapshotPolicy, SnapshotPublished,
    SnapshotPublishedTag, SnapshotReceipt, SnapshotTagName,
};

use crate::accounting_confirmation::confirm_accounting;
use crate::auto_tag::auto_tag_with_retry;
use crate::constants::{
    SPAN_SNAPSHOT_EXPORT, SPAN_SNAPSHOT_KV_WRITE, SPAN_SNAPSHOT_PUBLISHED, SPAN_SNAPSHOT_SECURITY_PIPELINE,
    TAG_RETRY_BACKOFF,
};
use crate::export::export_with_retry;
use crate::release_outcome::ReleaseOutcome;
use crate::release_tag::ReleaseTag;
use crate::security::{SecurityConfig, SecurityError, run_security_pipeline};
use crate::{BuilderConfig, ExportError, SnapshotKvStore};

/// Carries one released lease from its worker export to a moved tag.
pub(crate) struct ReleaseProcessor {
    pub(crate) nats: Client,
    pub(crate) kv: SnapshotKvStore,
    pub(crate) tag_store: Arc<dyn SnapshotTagStore>,
    pub(crate) accounting: Arc<SnapshotAccountingStore>,
    pub(crate) config: BuilderConfig,
}

impl ReleaseProcessor {
    pub(crate) async fn process(
        &self,
        event: &LeaseReleased,
        policy: &SnapshotPolicy,
        observed_at: SystemTime,
    ) -> Result<ReleaseOutcome, ExportError> {
        let lease_id = event.lease_id.as_str();
        let correlation_id = event.correlation_id.as_ref().map_or("unknown", |id| id.as_str());
        let default_registry = self.config.default_registry.as_str();
        let reservation = self
            .accounting
            .reserve(&event.tenant, &event.lease_id, &format!("release:{lease_id}"))
            .await
            .map_err(|reason| ExportError::Permanent(format!("snapshot reservation refused: {reason:?}")))?;

        let receipt = {
            let span = tracing::info_span!(SPAN_SNAPSHOT_EXPORT, lease_id, correlation_id);
            async {
                match self
                    .accounting
                    .binding(&reservation)
                    .await
                    .map_err(|reason| ExportError::Permanent(format!("snapshot binding unavailable: {reason:?}")))?
                {
                    Some(binding) => Ok(SnapshotReceipt {
                        digest: binding.reference.digest,
                        bytes: binding.bytes,
                        artifact_type: binding.reference.artifact_type,
                    }),
                    None => {
                        export_with_retry(
                            &self.nats,
                            self.config.dispatch_signer.as_deref(),
                            event,
                            default_registry,
                        )
                        .await
                    }
                }
            }
            .instrument(span)
            .await?
        };

        self.accounting
            .bind(
                &reservation,
                OciArtifactRef {
                    registry: default_registry.to_string(),
                    repository: policy.target_repository.clone(),
                    digest: receipt.digest.clone(),
                    artifact_type: receipt.artifact_type,
                },
                &receipt,
            )
            .await
            .map_err(|reason| ExportError::Permanent(format!("snapshot binding refused: {reason:?}")))?;

        let env_digest = if self.config.require_scan_pass || self.config.cosign_kms_uri.is_some() {
            let security_config = SecurityConfig {
                registry: default_registry.to_string(),
                insecure_registries: self.config.oci_insecure_registries.clone(),
                cosign_kms_uri: self.config.cosign_kms_uri.clone(),
                scan_cvss_threshold: self.config.scan_cvss_threshold,
                require_scan_pass: self.config.require_scan_pass,
                preparation: Default::default(),
            };
            let span = tracing::info_span!(
                SPAN_SNAPSHOT_SECURITY_PIPELINE,
                lease_id,
                digest = %receipt.digest,
                correlation_id,
            );
            run_security_pipeline(&security_config, policy, &receipt)
                .instrument(span)
                .await
                .map_err(security_error_to_export)?
        } else {
            None
        };

        let record = SnapshotKvRecord {
            lease_id: event.lease_id.clone(),
            tenant: event.tenant.clone(),
            digest: receipt.digest.clone(),
            artifact_type: receipt.artifact_type,
            registry: default_registry.to_string(),
            repository: policy.target_repository.clone(),
            parent: event.restored_from.clone(),
            created_at: Some(reservation.created_at),
            env_digest,
        };
        {
            let span = tracing::info_span!(SPAN_SNAPSHOT_KV_WRITE, lease_id, digest = %receipt.digest, correlation_id);
            self.kv
                .put(&record)
                .instrument(span)
                .await
                .map_err(|error| ExportError::Permanent(format!("failed to write snapshot KV record: {error}")))?;
        }

        let published = SnapshotPublished {
            lease_id: event.lease_id.clone(),
            digest: receipt.digest.clone(),
            artifact_type: receipt.artifact_type,
            tenant: event.tenant.clone(),
            bytes: receipt.bytes,
            generation: Some(reservation.generation.clone()),
            correlation_id: event.correlation_id.clone(),
            // Listeners and the tag store must use the same release ordering.
            tag: policy
                .tag_name
                .as_deref()
                .and_then(|name| SnapshotTagName::new(name).ok())
                .map(|name| SnapshotPublishedTag {
                    name,
                    set_at: event.released_at.unwrap_or(observed_at),
                    retention: policy.retention,
                }),
        };
        self.accounting
            .stage_publication(&published)
            .await
            .map_err(|reason| ExportError::Permanent(format!("snapshot seal refused: {reason:?}")))?;
        {
            let span = tracing::info_span!(SPAN_SNAPSHOT_PUBLISHED, lease_id, digest = %receipt.digest, correlation_id);
            self.accounting
                .enqueue(&published)
                .instrument(span)
                .await
                .map_err(|reason| ExportError::Permanent(format!("failed to persist SnapshotPublished: {reason:?}")))?;
        }

        let tag_failure = self.move_tag(event, policy, &receipt, &published, observed_at).await;

        info!(lease_id, digest = %receipt.digest, correlation_id, "snapshot published");
        Ok(ReleaseOutcome { receipt, tag_failure })
    }

    async fn move_tag(
        &self,
        event: &LeaseReleased,
        policy: &SnapshotPolicy,
        receipt: &SnapshotReceipt,
        published: &SnapshotPublished,
        observed_at: SystemTime,
    ) -> Option<ExportError> {
        match confirm_accounting(&self.nats, published).await {
            Err(reason) => Some(ExportError::TagNotMoved(format!(
                "snapshot accounting remains unconfirmed: {reason:?}"
            ))),
            Ok(SnapshotAccountingDisposition::Live) => {
                let tag_name = policy.tag_name.as_deref()?;
                let release = ReleaseTag {
                    event,
                    receipt,
                    tag_name,
                    retention: policy.retention,
                };
                auto_tag_with_retry(self.tag_store.as_ref(), &release, observed_at, TAG_RETRY_BACKOFF)
                    .await
                    .err()
                    .map(|error| {
                        ExportError::TagNotMoved(format!(
                            "snapshot {} is published but tag {tag_name} was not moved: {error}",
                            receipt.digest
                        ))
                    })
            }
            Ok(_) => None,
        }
    }
}

fn security_error_to_export(failure: SecurityError) -> ExportError {
    match failure {
        SecurityError::Trivy(error) => ExportError::ScanRejected(error.to_string()),
        SecurityError::Extraction(error) => ExportError::ScanRejected(error.to_string()),
        other => ExportError::Permanent(other.to_string()),
    }
}

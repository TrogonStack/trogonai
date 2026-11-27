use std::sync::OnceLock;

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use tokio::sync::OnceCell;
use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use crate::custody_bucket::{ensure_durable, open_custody_bucket};

use super::constants::{BUCKET, MAX_MANIFEST_BYTES};
use super::manifest::SnapshotManifest;
use super::manifests_error::SnapshotManifestsError;
use super::record_outcome::RecordOutcome;
use super::reference::{validate_reference, validate_reference_identity};

/// The per-tenant KV store of snapshot manifests, bound to JetStream on first use.
#[derive(Default)]
pub(crate) struct SnapshotManifests {
    context: OnceLock<jetstream::Context>,
    store: OnceCell<kv::Store>,
}

impl SnapshotManifests {
    /// Opens the bucket now so a missing JetStream surfaces before work starts.
    pub(crate) async fn ready(&self) -> Result<(), SnapshotManifestsError> {
        self.storage().await?;
        Ok(())
    }

    pub(crate) fn configure(&self, context: jetstream::Context) -> Result<(), SnapshotManifestsError> {
        self.context
            .set(context)
            .map_err(|_| SnapshotManifestsError::AlreadyConfigured)
    }

    async fn storage(&self) -> Result<&kv::Store, SnapshotManifestsError> {
        let context = self.context.get().ok_or(SnapshotManifestsError::NotConfigured)?;
        let store = self
            .store
            .get_or_try_init(|| {
                open_custody_bucket(
                    context,
                    kv::Config {
                        bucket: BUCKET.into(),
                        history: 1,
                        ..Default::default()
                    },
                )
            })
            .await?;
        ensure_durable(store).await?;
        Ok(store)
    }

    /// Writes `manifest` unless one already exists under the same tenant and reference.
    pub(crate) async fn record(&self, manifest: &SnapshotManifest) -> Result<RecordOutcome, SnapshotManifestsError> {
        manifest.validate(&manifest.tenant, &manifest.reference)?;
        let data = serde_json::to_vec(manifest).map_err(SnapshotManifestsError::Encode)?;
        if data.len() > MAX_MANIFEST_BYTES {
            return Err(SnapshotManifestsError::TooLarge);
        }
        let key = tenant_scoped_kv_key(&manifest.tenant, manifest.reference.id.as_str());
        match self.storage().await?.create(key, Bytes::from(data)).await {
            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => Ok(RecordOutcome::AlreadyExists),
            created => Ok(created.map(|_| RecordOutcome::Created)?),
        }
    }

    /// The manifest for `reference`, validated and unexpired, or `None` when none was recorded.
    pub(crate) async fn load(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
    ) -> Result<Option<SnapshotManifest>, SnapshotManifestsError> {
        validate_reference(reference)?;
        let manifest = self.load_for_delete(tenant, reference).await?;
        if let Some(manifest) = &manifest {
            manifest.validate(tenant, reference)?;
        }
        Ok(manifest)
    }

    /// Like [`Self::load`] but accepts an expired reference, so an expired snapshot can still be deleted.
    pub(crate) async fn load_for_delete(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
    ) -> Result<Option<SnapshotManifest>, SnapshotManifestsError> {
        validate_reference_identity(reference)?;
        let key = tenant_scoped_kv_key(tenant, reference.id.as_str());
        let Some(data) = self.storage().await?.get(key).await? else {
            return Ok(None);
        };
        if data.len() > MAX_MANIFEST_BYTES {
            return Err(SnapshotManifestsError::TooLarge);
        }
        let manifest: SnapshotManifest = serde_json::from_slice(&data).map_err(SnapshotManifestsError::Decode)?;
        manifest.validate_identity(tenant, reference)?;
        Ok(Some(manifest))
    }

    pub(crate) async fn remove(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
    ) -> Result<(), SnapshotManifestsError> {
        validate_reference_identity(reference)?;
        self.storage()
            .await?
            .delete(tenant_scoped_kv_key(tenant, reference.id.as_str()))
            .await?;
        Ok(())
    }
}

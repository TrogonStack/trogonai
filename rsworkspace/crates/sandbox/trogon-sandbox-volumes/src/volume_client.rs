use std::path::Path;
use std::time::Instant;

use bytes::Bytes;
use futures::StreamExt;
use object_store::{ObjectStoreExt, PutPayload};
use tokio::io::AsyncWriteExt;
use trogon_sandbox_contract::{Digest, VolumeMode};

use crate::store_handle::StoreHandle;
use crate::volume_mode_label::mode_label;
use crate::{
    ChangedManifest, CowHandle, KmsKeyId, PullSummary, PushSummary, VolumeClientConfig, VolumeError, VolumeMetrics,
    VolumePrefix, build_changed_manifest, compute_tree_digest,
};

/// A mode-aware client that moves a volume's files between a local directory and an object store.
pub struct VolumeClient {
    handle: StoreHandle,
    metrics: VolumeMetrics,
    mode: VolumeMode,
    quota_bytes: Option<u64>,
}

impl VolumeClient {
    pub fn new(config: VolumeClientConfig) -> Result<Self, VolumeError> {
        Self::with_mode(config, VolumeMode::ReadOnly)
    }

    pub fn with_mode(config: VolumeClientConfig, mode: VolumeMode) -> Result<Self, VolumeError> {
        Self::with_options(config, mode, VolumeMetrics::new()?, None)
    }

    pub fn with_options(
        config: VolumeClientConfig,
        mode: VolumeMode,
        metrics: VolumeMetrics,
        quota_bytes: Option<u64>,
    ) -> Result<Self, VolumeError> {
        Ok(Self {
            handle: StoreHandle::build(&config)?,
            metrics,
            mode,
            quota_bytes,
        })
    }

    pub fn bucket(&self) -> &str {
        &self.handle.bucket
    }

    pub fn mode(&self) -> VolumeMode {
        self.mode
    }

    pub fn metrics(&self) -> &VolumeMetrics {
        &self.metrics
    }

    pub fn kms_key_id(&self) -> Option<&KmsKeyId> {
        self.handle.kms_key_id.as_ref()
    }

    /// Downloads every object under `prefix` into `dest`, keeping their relative layout.
    pub async fn pull_to_dir(&self, prefix: &VolumePrefix, dest: &Path) -> Result<PullSummary, VolumeError> {
        let start = Instant::now();
        let bucket = self.handle.bucket.as_str();
        let mode = mode_label(self.mode);
        tokio::fs::create_dir_all(dest).await?;

        let mut objects = 0u64;
        let mut bytes = 0u64;
        let mut stream = self.handle.store.list(Some(prefix.object_path()));

        while let Some(meta) = stream.next().await {
            let meta = meta?;
            let key = prefix.relative_key(&meta.location)?;
            let local = key.to_path(dest);
            tokio::fs::create_dir_all(local.parent().unwrap_or(dest)).await?;
            let data = self.handle.store.get(&meta.location).await?.bytes().await?;
            let mut file = tokio::fs::File::create(&local).await?;
            file.write_all(&data).await?;
            file.flush().await?;
            objects += 1;
            bytes += data.len() as u64;
        }

        self.metrics
            .bytes_in_total
            .with_label_values(&[bucket, mode])
            .inc_by(bytes);
        self.metrics
            .pull_duration_seconds
            .with_label_values(&[bucket, mode])
            .observe(start.elapsed().as_secs_f64());

        tracing::info!(bucket, mode, prefix = %prefix, objects, bytes, "volume pull complete");

        Ok(PullSummary { objects, bytes })
    }

    /// Uploads the files listed in `manifest` from `src` under `prefix`.
    pub async fn push_changed(
        &self,
        prefix: &VolumePrefix,
        src: &Path,
        manifest: ChangedManifest,
    ) -> Result<PushSummary, VolumeError> {
        let start = Instant::now();
        let bucket = self.handle.bucket.as_str();
        let mode = mode_label(self.mode);

        let mut objects = 0u64;
        let mut bytes = 0u64;

        for entry in manifest.entries {
            let data = tokio::fs::read(entry.key.to_path(src)).await?;
            let len = data.len() as u64;
            if let Some(limit) = self.quota_bytes
                && bytes.saturating_add(len) > limit
            {
                return Err(VolumeError::QuotaExceeded {
                    bucket: bucket.to_owned(),
                    limit_bytes: limit,
                });
            }
            self.handle
                .store
                .put(
                    &prefix.object_path_for(&entry.key),
                    PutPayload::from_bytes(Bytes::from(data)),
                )
                .await?;
            objects += 1;
            bytes += len;
        }

        self.metrics
            .bytes_out_total
            .with_label_values(&[bucket, mode])
            .inc_by(bytes);
        self.metrics
            .push_duration_seconds
            .with_label_values(&[bucket, mode])
            .observe(start.elapsed().as_secs_f64());

        tracing::info!(bucket, mode, prefix = %prefix, objects, bytes, "volume push complete");

        Ok(PushSummary { objects, bytes })
    }

    /// Materializes the base prefix into `dest` and prepares a delta directory beside it.
    pub async fn cow_setup(
        &self,
        base_prefix: &VolumePrefix,
        lease_prefix: &VolumePrefix,
        dest: &Path,
    ) -> Result<CowHandle, VolumeError> {
        self.pull_to_dir(base_prefix, dest).await?;
        let delta_dir = dest.join(".cow-delta");
        tokio::fs::create_dir_all(&delta_dir).await?;
        Ok(CowHandle {
            base_prefix: base_prefix.clone(),
            lease_prefix: lease_prefix.clone(),
            delta_dir,
        })
    }

    /// Uploads the lease delta under the lease prefix and returns the digest identifying it.
    pub async fn cow_finalize(&self, handle: CowHandle) -> Result<Digest, VolumeError> {
        let manifest = build_changed_manifest(&handle.delta_dir, None)?;
        if manifest.entries.is_empty() {
            return compute_tree_digest(handle.delta_dir.parent().unwrap_or(Path::new(".")));
        }
        let digest = manifest.digest()?;
        self.push_changed(&handle.lease_prefix, &handle.delta_dir, manifest)
            .await?;
        Ok(digest)
    }

    /// Whether any object exists under `prefix`.
    pub async fn head_exists(&self, prefix: &VolumePrefix) -> Result<bool, VolumeError> {
        let mut stream = self.handle.store.list(Some(prefix.object_path()));
        match stream.next().await {
            Some(item) => {
                item?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn record_cache_hit(&self) {
        self.metrics
            .cache_hit_total
            .with_label_values(&[self.handle.bucket.as_str()])
            .inc();
    }
}

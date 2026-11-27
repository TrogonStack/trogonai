use std::collections::BTreeMap;

use async_trait::async_trait;
use bytes::Bytes;
use kube::core::DynamicObject;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStoreExt, PutPayload};
use serde_json::{Map, Value};
use tracing::info;
use trogon_sandbox_contract::OciArtifactRef;
use trogon_sandbox_cosign::{into_pull_error, verify_if_configured};
use trogon_sandbox_volumes::{VolumeBackend, VolumeClientConfig};

use crate::{KubeVirtOps, RestoreError, SnapshotService, WarmDiskSource, WarmDiskSourceError};

#[async_trait]
impl WarmDiskSource for SnapshotService {
    async fn data_volume(
        &self,
        ops: &KubeVirtOps,
        volume: &VolumeClientConfig,
        reference: &OciArtifactRef,
        dv_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<DynamicObject, WarmDiskSourceError> {
        self.restore(ops, volume, reference, dv_name, labels)
            .await
            .map_err(WarmDiskSourceError::new)
    }
}

impl SnapshotService {
    /// Pulls the snapshot, stages its disk in S3 and describes the DataVolume CDI imports it with.
    pub(crate) async fn restore(
        &self,
        ops: &KubeVirtOps,
        volume: &VolumeClientConfig,
        reference: &OciArtifactRef,
        dv_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<DynamicObject, RestoreError> {
        let VolumeBackend::S3 {
            bucket,
            region,
            endpoint,
        } = &volume.backend
        else {
            return Err(RestoreError::UnsupportedBackend);
        };
        verify_if_configured(ops.config().cosign_verifier.as_ref(), reference)
            .await
            .map_err(|error| RestoreError::Verify(Box::new(into_pull_error(error))))?;
        info!(digest = %reference.digest, "pulling the snapshot disk");
        let layers = self.registry.pull(reference).await?;
        let layer = layers.into_iter().next().ok_or(RestoreError::NoLayers)?;
        let key = format!(
            "{}{}/{}.qcow2",
            ops.config().restore_bucket_prefix,
            reference.digest.hex(),
            dv_name
        );
        let store = self.stores.open(bucket, region, endpoint.as_ref())?;
        let size = layer.data.len();
        store
            .put(
                &ObjectPath::from(key.trim_start_matches('/')),
                PutPayload::from_bytes(Bytes::from(layer.data)),
            )
            .await?;
        let url = format!("s3://{bucket}/{key}", bucket = bucket.as_str());
        info!(%url, bytes = size, "staged the snapshot disk for import");
        let source = s3_source(url, ops.config().s3_secret_name.as_deref());
        Ok(ops.build_data_volume_spec(dv_name, source, labels))
    }
}

fn s3_source(url: String, secret: Option<&str>) -> Value {
    let mut s3 = Map::new();
    s3.insert("url".to_owned(), Value::String(url));
    if let Some(secret) = secret {
        s3.insert("secretRef".to_owned(), Value::String(secret.to_owned()));
    }
    let mut source = Map::new();
    source.insert("s3".to_owned(), Value::Object(s3));
    Value::Object(source)
}

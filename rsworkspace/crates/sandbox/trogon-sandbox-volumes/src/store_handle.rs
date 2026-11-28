use std::sync::Arc;

use object_store::ObjectStore;
use object_store::local::LocalFileSystem;

use object_store::aws::AmazonS3Builder;

use crate::{KmsKeyId, VolumeBackend, VolumeClientConfig, VolumeEndpoint, VolumeError, VolumeRegion};

pub(crate) struct StoreHandle {
    pub(crate) store: Arc<dyn ObjectStore>,
    pub(crate) bucket: String,
    pub(crate) kms_key_id: Option<KmsKeyId>,
}

impl StoreHandle {
    pub(crate) fn build(config: &VolumeClientConfig) -> Result<Self, VolumeError> {
        let backend_name = config.backend_name();
        let build_error = |source| VolumeError::StoreBuild {
            backend: backend_name,
            source,
        };
        let store: Arc<dyn ObjectStore> = match &config.backend {
            VolumeBackend::S3 {
                bucket,
                region,
                endpoint,
            } => {
                let builder = s3_builder(
                    object_store::aws::AmazonS3Builder::from_env(),
                    bucket.as_str(),
                    region,
                    endpoint.as_ref(),
                );
                Arc::new(builder.build().map_err(build_error)?)
            }
            VolumeBackend::Gcs { bucket } => Arc::new(
                object_store::gcp::GoogleCloudStorageBuilder::new()
                    .with_bucket_name(bucket.as_str())
                    .build()
                    .map_err(build_error)?,
            ),
            VolumeBackend::AzureBlob { container, account } => Arc::new(
                object_store::azure::MicrosoftAzureBuilder::new()
                    .with_container_name(container.as_str())
                    .with_account(account.as_str())
                    .build()
                    .map_err(build_error)?,
            ),
            VolumeBackend::LocalFs { root } => {
                std::fs::create_dir_all(root)?;
                Arc::new(LocalFileSystem::new_with_prefix(root).map_err(build_error)?)
            }
        };
        Ok(Self {
            store,
            bucket: config.bucket_label(),
            kms_key_id: config.kms_key_id.clone(),
        })
    }
}

/// Points `base`, which may carry settings from the environment, at the volume's bucket. Plain `http` is allowed
/// only for an endpoint that is configured and written with the `http` scheme; the environment cannot switch it on.
fn s3_builder(
    base: AmazonS3Builder,
    bucket: &str,
    region: &VolumeRegion,
    endpoint: Option<&VolumeEndpoint>,
) -> AmazonS3Builder {
    let builder = base
        .with_bucket_name(bucket)
        .with_region(region.as_str())
        .with_allow_http(endpoint.is_some_and(VolumeEndpoint::is_plain_http));
    match endpoint {
        Some(endpoint) => builder.with_endpoint(endpoint.as_str()),
        None => builder,
    }
}

#[cfg(test)]
mod tests;

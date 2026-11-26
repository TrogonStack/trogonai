use crate::{KmsKeyId, VolumeBackend};

#[derive(Debug, Clone)]
pub struct VolumeClientConfig {
    pub backend: VolumeBackend,
    pub kms_key_id: Option<KmsKeyId>,
}

impl VolumeClientConfig {
    pub fn bucket_label(&self) -> String {
        match &self.backend {
            VolumeBackend::S3 { bucket, .. } => bucket.to_string(),
            VolumeBackend::Gcs { bucket } => bucket.to_string(),
            VolumeBackend::AzureBlob { container, .. } => container.to_string(),
            VolumeBackend::LocalFs { root } => root.to_string_lossy().into_owned(),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match &self.backend {
            VolumeBackend::S3 { .. } => "s3",
            VolumeBackend::Gcs { .. } => "gcs",
            VolumeBackend::AzureBlob { .. } => "azure",
            VolumeBackend::LocalFs { .. } => "local_fs",
        }
    }
}

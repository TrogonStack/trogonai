use trogon_sandbox_contract::{NativeSnapshotId, OciImageRef, ProviderKind, SandboxImage};

use crate::ModalProviderError;
use crate::constants::{IMAGE_DIGEST_ENV, IMAGE_ENV, IMAGE_TAG_ENV, SNAPSHOT_UNSUPPORTED_REASON};

/// What a Modal sandbox is created from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModalImage {
    Registry(OciImageRef),
    /// A `modal.Image` id from a filesystem snapshot Modal took itself.
    Snapshot(NativeSnapshotId),
}

impl ModalImage {
    /// Only registry images and Modal's own snapshots are accepted: Modal owns its image lineage and this worker
    /// does not mirror it into the Trogon snapshot store.
    pub(crate) fn resolve(image: &SandboxImage) -> Result<Self, ModalProviderError> {
        match image {
            SandboxImage::NativeSnapshot { reference } if reference.provider == ProviderKind::Modal => {
                Ok(Self::Snapshot(reference.id.clone()))
            }
            SandboxImage::NativeSnapshot { reference } => Err(ModalProviderError::InvalidSpec(format!(
                "a {:?} native snapshot cannot seed a Modal sandbox",
                reference.provider
            ))),
            SandboxImage::Registry { reference, .. } => Ok(Self::Registry(reference.clone())),
            SandboxImage::Snapshot { .. }
            | SandboxImage::SnapshotTag { .. }
            | SandboxImage::HyperlightModule { .. } => Err(ModalProviderError::SnapshotUnsupported {
                provider: ProviderKind::Modal,
                reason: SNAPSHOT_UNSUPPORTED_REASON.to_owned(),
            }),
        }
    }
}

/// The variables that tell the guest which image it runs.
pub(crate) fn image_env_vars(reference: &OciImageRef) -> Vec<(String, String)> {
    let mut vars = vec![(IMAGE_ENV.to_owned(), reference.allowlist_string())];
    if let Some(tag) = &reference.tag {
        vars.push((IMAGE_TAG_ENV.to_owned(), tag.clone()));
    }
    if let Some(digest) = &reference.digest {
        vars.push((IMAGE_DIGEST_ENV.to_owned(), digest.as_str().to_owned()));
    }
    vars
}

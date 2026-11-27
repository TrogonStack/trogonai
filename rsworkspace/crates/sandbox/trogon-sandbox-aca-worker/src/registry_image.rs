use trogon_sandbox_contract::{OciImageRef, SandboxImage};

use crate::AcaError;
use crate::constants::SNAPSHOT_UNSUPPORTED_REASON;

/// The container image a lease boots: its own registry reference, or `default_image` when the
/// reference names no repository. Snapshots cannot be restored on Dynamic Sessions.
pub fn resolve_registry_image(
    image: &SandboxImage,
    default_image: Option<&OciImageRef>,
) -> Result<OciImageRef, AcaError> {
    match image {
        SandboxImage::Registry { reference, .. } if reference.repository.is_empty() => {
            default_image.cloned().ok_or_else(|| {
                AcaError::InvalidSpec("ACA provider requires a non-empty container image reference".to_owned())
            })
        }
        SandboxImage::Registry { reference, .. } => Ok(reference.clone()),
        SandboxImage::Snapshot { .. }
        | SandboxImage::SnapshotTag { .. }
        | SandboxImage::NativeSnapshot { .. }
        | SandboxImage::HyperlightModule { .. } => {
            Err(AcaError::SnapshotUnsupported(SNAPSHOT_UNSUPPORTED_REASON.to_owned()))
        }
    }
}

/// The environment variables that tell code in the session which image it runs in.
pub fn image_env_vars(reference: &OciImageRef) -> Vec<(&'static str, String)> {
    let mut vars = vec![("SANDBOX_ACA_IMAGE", reference.allowlist_string())];
    vars.extend(reference.tag.iter().map(|tag| ("SANDBOX_ACA_IMAGE_TAG", tag.clone())));
    vars.extend(
        reference
            .digest
            .iter()
            .map(|digest| ("SANDBOX_ACA_IMAGE_DIGEST", digest.as_str().to_owned())),
    );
    vars
}

//! Read-only views over a [`SandboxImage`] and an [`OperationRequest`] that the
//! broker needs for allowlist matching, logging and operation bookkeeping.

use trogon_sandbox_contract::{Digest, OciImageRef, OperationKind, OperationRequest, SandboxImage};

/// Names an image the way image policies and logs refer to it.
pub(crate) trait SandboxImageLabels {
    /// A short label for logs and error messages.
    fn display_label(&self) -> String;
    /// Every string an image allowlist may match this image against.
    fn allowlist_match_strings(&self) -> Vec<String>;
    /// The registry image this boots from, when it names one.
    fn primary_oci_image_ref(&self) -> Option<&OciImageRef>;
    /// The digest of a snapshot image, when it is one.
    fn snapshot_digest(&self) -> Option<&Digest>;
}

impl SandboxImageLabels for SandboxImage {
    fn display_label(&self) -> String {
        match self {
            Self::Registry { reference, .. } => reference.allowlist_string(),
            Self::Snapshot { reference, .. } | Self::HyperlightModule { reference } => reference.allowlist_string(),
            Self::SnapshotTag { tag, .. } => format!("tag:{}:{}", tag.tenant, tag.name),
            Self::NativeSnapshot { reference } => {
                format!("native:{:?}:{}", reference.provider, reference.id)
            }
        }
    }

    fn allowlist_match_strings(&self) -> Vec<String> {
        match self {
            Self::Snapshot { reference, parent } => {
                let mut out = vec![reference.allowlist_string()];
                out.extend(parent.iter().map(OciImageRef::allowlist_string));
                out
            }
            _ => vec![self.display_label()],
        }
    }

    fn primary_oci_image_ref(&self) -> Option<&OciImageRef> {
        match self {
            Self::Registry { reference, .. } => Some(reference),
            Self::Snapshot { parent, .. } => parent.as_ref(),
            Self::SnapshotTag { .. } | Self::HyperlightModule { .. } | Self::NativeSnapshot { .. } => None,
        }
    }

    fn snapshot_digest(&self) -> Option<&Digest> {
        match self {
            Self::Snapshot { reference, .. } => Some(&reference.digest),
            _ => None,
        }
    }
}

/// The wire kind of an operation request. A create whose image is a snapshot
/// is a restore.
pub(crate) fn operation_kind(request: &OperationRequest) -> OperationKind {
    match request {
        OperationRequest::Create { request, .. } => {
            if matches!(request.spec.image(), SandboxImage::Snapshot { .. }) {
                OperationKind::Restore
            } else {
                OperationKind::Create
            }
        }
        OperationRequest::Pause { .. } => OperationKind::Pause,
        OperationRequest::Resume => OperationKind::Resume,
        OperationRequest::Suspend { .. } => OperationKind::Suspend,
        OperationRequest::Wake => OperationKind::Wake,
        OperationRequest::Delete => OperationKind::Delete,
        OperationRequest::Rollback { .. } => OperationKind::Rollback,
    }
}

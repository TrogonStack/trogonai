use serde::{Deserialize, Serialize};

use crate::{ArtifactType, NativeSnapshotRef, OciArtifactRef, OciImageRef, PullSecretId, SnapshotTag};

/// What a lease boots from, tagged by `kind`.
///
/// `Registry` is an image pulled from a registry, optionally with a pull secret. `Snapshot` is a digest-pinned
/// artifact, with the image it was taken from when known. `SnapshotTag` is a named tag the broker resolves to a
/// digest at lease time, optionally falling back to `create_from` while the tag has no snapshot yet.
/// `HyperlightModule` is a module artifact. `NativeSnapshot` is a snapshot a provider holds itself, which only a
/// worker of that provider can restore.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SandboxImage {
    Registry {
        reference: OciImageRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pull_secret: Option<PullSecretId>,
    },
    Snapshot {
        reference: OciArtifactRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<OciImageRef>,
    },
    SnapshotTag {
        tag: SnapshotTag,
        artifact_type: ArtifactType,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        create_from: Option<Box<SandboxImage>>,
    },
    HyperlightModule {
        reference: OciArtifactRef,
    },
    NativeSnapshot {
        reference: NativeSnapshotRef,
    },
}

impl SandboxImage {
    /// Reports whether this is a registry image with no repository, which names nothing to boot.
    pub fn is_unspecified(&self) -> bool {
        matches!(self, Self::Registry { reference, .. } if reference.repository.is_empty())
    }
}

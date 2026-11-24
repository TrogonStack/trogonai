use serde::{Deserialize, Serialize};

use crate::{CloneAuth, Digest, EmptyObject, HostMountAccess, VolumeMode};

/// What a lease's workspace starts from, serialized as a JSON object keyed by the variant name.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSource {
    /// A Git checkout of `sha`. `base_sha` names the commit a change is measured against, and is
    /// sent as `null` when absent.
    Clone {
        url: String,
        sha: String,
        base_sha: Option<String>,
        auth: CloneAuth,
    },
    /// A directory on the worker's host.
    HostMount {
        path: String,
        #[serde(default, rename = "readonly")]
        access: HostMountAccess,
    },
    /// An S3 prefix mounted in `mode`, optionally seeded from a snapshot.
    S3Volume {
        bucket: String,
        prefix: String,
        mode: VolumeMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot: Option<Digest>,
    },
    /// An empty workspace, serialized as `{"empty":{}}`.
    Empty(EmptyObject),
}

impl WorkspaceSource {
    pub fn empty() -> Self {
        Self::Empty(EmptyObject::default())
    }
}

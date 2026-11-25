use serde::{Deserialize, Serialize};

use crate::S3BucketName;

/// Where a remote mount reads from, tagged by `backend`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum RemoteMountSource {
    S3 { bucket: S3BucketName, prefix: String },
}

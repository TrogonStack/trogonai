use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// A worker's answer to a [`CreateSandboxGroupRequest`](crate::CreateSandboxGroupRequest), tagged by
/// `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CreateSandboxGroupResponse {
    Created {
        shared_dir: String,
    },
    AlreadyExists {
        shared_dir: String,
    },
    /// A required binary, such as `groupadd`, or `/bin/bash` is absent from the image.
    MissingDependency {
        tool: String,
    },
    Denied(LeaseError),
}

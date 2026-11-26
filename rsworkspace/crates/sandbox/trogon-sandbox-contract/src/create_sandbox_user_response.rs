use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// A worker's answer to a [`CreateSandboxUserRequest`](crate::CreateSandboxUserRequest), tagged by
/// `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CreateSandboxUserResponse {
    Created {
        home_dir: String,
    },
    AlreadyExists {
        home_dir: String,
    },
    /// A required binary, such as `useradd`, or `/bin/bash` is absent from the image.
    MissingDependency {
        tool: String,
    },
    Denied(LeaseError),
}

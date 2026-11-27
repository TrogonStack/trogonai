use serde::{Deserialize, Serialize};

use crate::SandboxEntityId;

/// How a client names an entity: by its id, or by its tenant-scoped name.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityRef {
    Id(SandboxEntityId),
    Name(String),
}

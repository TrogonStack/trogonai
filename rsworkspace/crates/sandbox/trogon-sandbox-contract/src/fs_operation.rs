use serde::{Deserialize, Serialize};

/// What a sandboxed process tried to do to a path.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsOperation {
    Read,
    Write,
    Create,
    Delete,
    Rename,
    Link,
    SetMetadata,
}

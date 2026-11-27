use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{FsOperation, LeaseId, LiteralPath};

/// One filesystem operation the sandbox refused.
///
/// Linux observes the attempt and compares it with the paths the lease may write, so only write-intent operations
/// are reported there; macOS reads the kernel's own denials, reads included. The path is what the sandboxed process
/// asked for, normalized lexically, and is a diagnostic hint, never an input to a policy decision. `at` uses
/// serde's `SystemTime` encoding of `secs_since_epoch` and `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct FsViolation {
    pub lease_id: LeaseId,
    pub path: LiteralPath,
    pub operation: FsOperation,
    pub at: SystemTime,
}

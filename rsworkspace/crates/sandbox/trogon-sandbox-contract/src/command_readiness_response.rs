use serde::{Deserialize, Serialize};

use crate::{
    CommandReadinessDeniedReason, CommandReadinessRequest, CommandReadinessRuntime, CommandReadinessUnavailableReason,
    CommandReadinessUnsupportedReason,
};

/// A worker's answer to a [`CommandReadinessRequest`], tagged by `outcome`.
///
/// `ready` reports the supervisor the sandbox runs. `unsupported` and `unavailable` echo the binding with the
/// reason the sandbox cannot run commands, and `denied` refuses the request without echoing it. Deserialization
/// rejects unknown fields.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandReadinessResponse {
    Ready {
        binding: CommandReadinessRequest,
        runtime: CommandReadinessRuntime,
    },
    Unsupported {
        binding: CommandReadinessRequest,
        reason: CommandReadinessUnsupportedReason,
    },
    Unavailable {
        binding: CommandReadinessRequest,
        reason: CommandReadinessUnavailableReason,
    },
    Denied {
        reason: CommandReadinessDeniedReason,
    },
}

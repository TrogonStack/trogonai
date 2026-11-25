use serde::{Deserialize, Serialize};

use crate::{ExecutionId, LeaseId, StopCommandRequest};

/// A client request to run a previously started detached command again under a new execution id.
///
/// The command runs from the request it started with: same argv, cwd, env, and process spec. A command still running
/// is stopped first, like [`StopCommandRequest`]. A named process keeps its name and readiness probe, so waiting for
/// readiness on the new execution id probes it again. An omitted `grace_seconds` reads as
/// [`StopCommandRequest::DEFAULT_GRACE_SECONDS`].
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RestartCommandRequest {
    pub lease_id: LeaseId,
    pub execution_id: ExecutionId,
    #[serde(default = "default_grace_seconds")]
    pub grace_seconds: u32,
}

fn default_grace_seconds() -> u32 {
    StopCommandRequest::DEFAULT_GRACE_SECONDS
}

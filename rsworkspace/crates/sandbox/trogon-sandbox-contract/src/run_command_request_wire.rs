use serde::{Deserialize, Serialize};

use crate::{
    CommandDeadline, CommandDetachment, CommandEnv, LaunchCancellation, LaunchKey, LeaseId, OutputRetention,
    ProcessSpec, RedactValues, SandboxUserName, StdinMode, StreamedOutput,
};

fn default_timeout_seconds() -> u32 {
    crate::constants::DEFAULT_COMMAND_TIMEOUT_SECONDS
}

/// The JSON shape of a [`RunCommandRequest`](crate::RunCommandRequest) before its options are checked together.
#[derive(Serialize, Deserialize)]
pub(crate) struct RunCommandRequestWire {
    pub(crate) lease_id: LeaseId,
    pub(crate) command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) env: CommandEnv,
    #[serde(default = "default_timeout_seconds")]
    pub(crate) timeout_seconds: u32,
    #[serde(default)]
    pub(crate) detached: CommandDetachment,
    #[serde(default)]
    pub(crate) stream_output: StreamedOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run_as_user: Option<SandboxUserName>,
    #[serde(default)]
    pub(crate) stdin: StdinMode,
    #[serde(default)]
    pub(crate) output_retention: OutputRetention,
    #[serde(default)]
    pub(crate) deadline: CommandDeadline,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_output_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) process: Option<ProcessSpec>,
    #[serde(default, skip_serializing_if = "RedactValues::is_empty")]
    pub(crate) redact_values: RedactValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) launch_key: Option<LaunchKey>,
    #[serde(default, skip_serializing_if = "LaunchCancellation::is_not_required")]
    pub(crate) require_launch_cancellation: LaunchCancellation,
}

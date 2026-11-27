use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{LeaseId, OutputRetention, StdinMode};

/// Builds the argv of a detached command for one execution id, which it puts
/// in the command's environment as the signal marker.
pub type CommandBuilder = Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>;

/// The request a detached command was started with, kept so `restart_command`
/// can run it again under a new execution id.
#[derive(Clone)]
pub(crate) struct StoredLaunch {
    pub(crate) lease_id: LeaseId,
    pub(crate) build: CommandBuilder,
    pub(crate) stdin: StdinMode,
    pub(crate) retention: OutputRetention,
    pub(crate) timeout: Duration,
    pub(crate) max_output_bytes: usize,
}

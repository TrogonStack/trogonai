use super::*;

/// The request a detached command was started with, kept so `restart_command`
/// can run it again. Held in worker memory only: it carries the redaction
/// values, which are secrets and must not be written anywhere durable.
#[derive(Clone)]
pub(super) struct StoredLaunch {
    pub(super) controlled_request: Option<Box<trogon_sandbox_contract::RunCommandRequest>>,
    pub(super) lease_id: LeaseId,
    pub(super) command: Vec<String>,
    pub(super) cwd: Option<String>,
    pub(super) env: std::collections::BTreeMap<String, String>,
    pub(super) timeout_seconds: u32,
    pub(super) run_as_user: Option<SandboxUserName>,
    pub(super) stream_output: bool,
    pub(super) stdin_mode: StdinMode,
    pub(super) retention: OutputRetention,
    pub(super) deadline: CommandDeadline,
    pub(super) max_output_bytes: usize,
    pub(super) process: Option<ProcessSpec>,
    pub(super) redaction: RedactionSet,
}

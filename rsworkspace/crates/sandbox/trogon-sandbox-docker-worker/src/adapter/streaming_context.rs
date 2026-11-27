use super::*;

/// Threaded into [`DockerWorkerProvider::exec_collect`] only when the caller
/// requested [`trogon_sandbox_contract::RunCommandRequest::stream_output`], so
/// non-streaming callers (write_files, read_file, create_user, kill exec,
/// etc.) keep passing `None` and are otherwise unaffected. Each chunk is
/// published as a [`trogon_sandbox_contract::CommandOutputChunk`] to
/// `sandbox.evidence.{lease_id}.command.{execution_id}` via jetstream,
/// matching [`crate::nats_lifecycle::NatsLifecycleEmitter`]'s publish
/// convention.
pub(super) struct StreamingContext {
    pub(super) js: jetstream::Context,
    pub(super) subject: String,
    pub(super) exit_subject: String,
    pub(super) execution_id: ExecutionId,
}

impl StreamingContext {
    pub(super) fn new(js: jetstream::Context, lease_id: &LeaseId, execution_id: &ExecutionId) -> Self {
        Self {
            js,
            subject: format!(
                "sandbox.evidence.{}.{}",
                lease_id.as_str(),
                trogon_sandbox_protocol::subjects::evidence_command_suffix(execution_id)
            ),
            exit_subject: trogon_sandbox_protocol::subjects::command_exit_subject(lease_id, execution_id),
            execution_id: execution_id.clone(),
        }
    }
}

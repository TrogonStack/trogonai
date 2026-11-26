use std::time::Duration;

use trogon_sandbox_contract::{
    CommandDeadline, CommandExecution, CommandOutput, ExecutionId, LaunchCancellation, LaunchKey, LeaseId, ProcessSpec,
    RunCommandRequest, RunCommandResponse, StdinMode,
};
use trogon_sandbox_protocol::subjects;
use uuid::Uuid;

use crate::command_control::invalid_spec;
use crate::sandbox_client::decode;
use crate::{ClientError, CommandOptions, SandboxClient, TransportError};

/// How long the client waits for a detached command to start.
const DETACHED_START_TIMEOUT: Duration = Duration::from_secs(40);

impl SandboxClient {
    /// Runs `command` inside `lease_id`'s already-running sandbox and returns its output.
    pub async fn run_command(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        options: CommandOptions,
    ) -> Result<CommandOutput, ClientError> {
        let request = launch_request(lease_id, command, options, CommandExecution::Attached);
        match self.send_run_command(request).await? {
            RunCommandResponse::Completed(output) => Ok(output),
            RunCommandResponse::Started(_) => Err(unexpected_reply("a started reply to an attached run_command")),
            RunCommandResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Starts `command` inside `lease_id`'s already-running sandbox without waiting for it to finish, returning an
    /// execution id to poll via [`Self::poll_command`]. With [`StdinMode::Interactive`] the command's stdin stays
    /// open for [`Self::write_stdin`].
    pub async fn run_command_detached(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        options: CommandOptions,
        stdin: StdinMode,
        deadline: CommandDeadline,
    ) -> Result<ExecutionId, ClientError> {
        self.run_process(lease_id, command, options, stdin, deadline, None)
            .await
    }

    /// [`Self::run_command_detached`] with an optional [`ProcessSpec`]: a caller-chosen name, unique per lease, and
    /// a readiness probe. Starting a name that is already running returns the existing execution id instead of a
    /// second process, so a retried call is safe.
    pub async fn run_process(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        options: CommandOptions,
        stdin: StdinMode,
        deadline: CommandDeadline,
        process: Option<ProcessSpec>,
    ) -> Result<ExecutionId, ClientError> {
        let execution = CommandExecution::Detached {
            stdin,
            deadline,
            process,
        };
        let request = launch_request(lease_id, command, options, execution);
        match self.send_run_command(request).await? {
            RunCommandResponse::Started(started) => Ok(started.execution_id),
            RunCommandResponse::Completed(_) => Err(unexpected_reply("a completed reply to a detached run_command")),
            RunCommandResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Sends `request` as is and returns the broker's raw reply, for callers such as gateways that relay a request
    /// built elsewhere. A request without a launch key gets a fresh one. If the reply is lost to a timeout or
    /// transport error, the request is sent once more under the same key: the broker replays the first launch for a
    /// key it already saw, so the retry recovers the reply without running the command twice.
    pub async fn send_run_command(&self, mut request: RunCommandRequest) -> Result<RunCommandResponse, ClientError> {
        let subject = subjects::lease_run_command(&request.lease_id);
        let timeout = match request.execution {
            // A detached start only returns the process id, so its wait does not follow the command timeout.
            // Longer than the broker's own wait on a slow provider, so the broker's answer arrives first.
            CommandExecution::Detached { .. } => DETACHED_START_TIMEOUT,
            // Longer than the broker's own grace past the command timeout, so a command that runs out its timeout
            // comes back as the broker's reply.
            CommandExecution::Attached => Duration::from_secs(u64::from(request.timeout_seconds.max(1)) + 10),
        };
        if request.launch_key.is_none() {
            let key = LaunchKey::new(Uuid::new_v4().simple().to_string()).map_err(invalid_spec)?;
            request.launch_key = Some(key);
        }
        let payload = serde_json::to_vec(&request).map_err(TransportError::Encode)?;
        let reply = match self.call_bytes(&subject, payload.clone(), timeout).await {
            Ok(bytes) => bytes,
            Err(_) => self.call_bytes(&subject, payload, timeout).await?,
        };
        decode(&reply)
    }
}

fn launch_request(
    lease_id: &LeaseId,
    command: Vec<String>,
    options: CommandOptions,
    execution: CommandExecution,
) -> RunCommandRequest {
    RunCommandRequest {
        lease_id: lease_id.clone(),
        command,
        cwd: options.cwd,
        env: options.env,
        timeout_seconds: options.timeout_seconds,
        stream_output: options.stream_output,
        run_as_user: options.run_as_user,
        output_retention: options.output_retention,
        max_output_bytes: None,
        redact_values: options.redact_values,
        launch_key: None,
        require_launch_cancellation: LaunchCancellation::NotRequired,
        execution,
    }
}

fn unexpected_reply(reply: &str) -> ClientError {
    ClientError::Transport(TransportError::UnexpectedReply {
        operation: "run_command",
        reply: reply.to_string(),
    })
}

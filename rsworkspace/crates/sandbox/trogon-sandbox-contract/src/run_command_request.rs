use serde::{Deserialize, Serialize};

use crate::run_command_request_wire::RunCommandRequestWire;
use crate::{
    CommandDeadline, CommandDetachment, CommandEnv, CommandExecution, LaunchCancellation, LaunchKey, LeaseId,
    OutputRetention, RedactValues, RunCommandRequestError, SandboxUserName, StdinMode, StreamedOutput,
};

/// A request to run an arbitrary command inside an already-running lease's sandbox.
///
/// A broker sends it to every worker for the lease's provider, and only the worker holding `lease_id` replies.
///
/// Deserialization rejects a request that asks for interactive stdin, a lease deadline, or a process spec without
/// `detached: true`. In this type those options live in [`CommandExecution::Detached`], so an attached command cannot
/// carry them. The environment is a [`CommandEnv`], which cannot name the variable a worker reserves.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RunCommandRequestWire", into = "RunCommandRequestWire")]
pub struct RunCommandRequest {
    pub lease_id: LeaseId,
    pub command: Vec<String>,
    /// Directory the command runs in. Absent, each worker keeps its own default. A relative directory joins that
    /// default.
    pub cwd: Option<String>,
    pub env: CommandEnv,
    /// How long the command may run, in seconds. An omitted value reads as
    /// [`RunCommandRequest::DEFAULT_TIMEOUT_SECONDS`].
    pub timeout_seconds: u32,
    /// Whether the worker publishes incremental output chunks while the command runs. Polling and the final response
    /// behave the same either way.
    pub stream_output: StreamedOutput,
    /// Per-call user override for this command's execution. When set, the worker runs the command as this user and,
    /// if `cwd` is `None`, defaults `cwd` to `/home/<user>`. `run_as_user: "root"` is the sandbox's sudo path. `None`
    /// runs the command as the lease's configured user.
    pub run_as_user: Option<SandboxUserName>,
    /// Whether the worker keeps the command's stdout for polling. See [`OutputRetention::StreamOnly`].
    pub output_retention: OutputRetention,
    /// Most bytes of each of stdout and stderr the worker retains for this command. Output beyond it is dropped from
    /// the middle and the response sets [`CommandOutput::truncated`](crate::CommandOutput::truncated). `None` leaves
    /// the worker's default, and a broker may clamp the value to its own ceiling. Streamed chunks are not affected.
    pub max_output_bytes: Option<u64>,
    /// Literal values the worker replaces with `[REDACTED]` in everything it returns for this command: stdout,
    /// stderr, streamed chunks, and error text. They are per command because the literals a command can echo are the
    /// ones it was handed, which differ call to call. Only exact bytes are matched, so encoded or transformed values
    /// are not caught. A worker ignores values too short to be worth redacting. The values are secrets, so `Debug`
    /// hides them, and the request must never be logged or persisted. See [`RedactValues`] for the limits.
    pub redact_values: RedactValues,
    /// Caller-chosen key that makes a retried launch safe. When the broker already saw this key for this lease, it
    /// replays the first launch's response, or waits for it while that launch is in flight, instead of running the
    /// command again. Reusing a key for a different command is refused. Keys are scoped to the lease.
    pub launch_key: Option<LaunchKey>,
    /// Whether the launch must be cancelable.
    pub require_launch_cancellation: LaunchCancellation,
    /// Whether the command runs to completion or detached, with the options only a detached command may use.
    pub execution: CommandExecution,
}

impl RunCommandRequest {
    /// The timeout applied when `timeout_seconds` is omitted, in seconds.
    pub const DEFAULT_TIMEOUT_SECONDS: u32 = crate::constants::DEFAULT_COMMAND_TIMEOUT_SECONDS;
}

impl TryFrom<RunCommandRequestWire> for RunCommandRequest {
    type Error = RunCommandRequestError;

    fn try_from(wire: RunCommandRequestWire) -> Result<Self, Self::Error> {
        let execution = match wire.detached {
            CommandDetachment::Detached => CommandExecution::Detached {
                stdin: wire.stdin,
                deadline: wire.deadline,
                process: wire.process,
            },
            CommandDetachment::Attached => {
                if wire.stdin == StdinMode::Interactive {
                    return Err(RunCommandRequestError::InteractiveStdinRequiresDetached);
                }
                if wire.deadline == CommandDeadline::Lease {
                    return Err(RunCommandRequestError::LeaseDeadlineRequiresDetached);
                }
                if wire.process.is_some() {
                    return Err(RunCommandRequestError::ProcessRequiresDetached);
                }
                CommandExecution::Attached
            }
        };
        Ok(Self {
            lease_id: wire.lease_id,
            command: wire.command,
            cwd: wire.cwd,
            env: wire.env,
            timeout_seconds: wire.timeout_seconds,
            stream_output: wire.stream_output,
            run_as_user: wire.run_as_user,
            output_retention: wire.output_retention,
            max_output_bytes: wire.max_output_bytes,
            redact_values: wire.redact_values,
            launch_key: wire.launch_key,
            require_launch_cancellation: wire.require_launch_cancellation,
            execution,
        })
    }
}

impl From<RunCommandRequest> for RunCommandRequestWire {
    fn from(request: RunCommandRequest) -> Self {
        let (detached, stdin, deadline, process) = match request.execution {
            CommandExecution::Attached => (
                CommandDetachment::Attached,
                StdinMode::default(),
                CommandDeadline::default(),
                None,
            ),
            CommandExecution::Detached {
                stdin,
                deadline,
                process,
            } => (CommandDetachment::Detached, stdin, deadline, process),
        };
        Self {
            lease_id: request.lease_id,
            command: request.command,
            cwd: request.cwd,
            env: request.env,
            timeout_seconds: request.timeout_seconds,
            detached,
            stream_output: request.stream_output,
            run_as_user: request.run_as_user,
            stdin,
            output_retention: request.output_retention,
            deadline,
            max_output_bytes: request.max_output_bytes,
            process,
            redact_values: request.redact_values,
            launch_key: request.launch_key,
            require_launch_cancellation: request.require_launch_cancellation,
        }
    }
}

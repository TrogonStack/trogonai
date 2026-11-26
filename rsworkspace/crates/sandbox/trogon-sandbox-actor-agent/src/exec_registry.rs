use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};
use trogon_sandbox_contract::{
    ExecutionId, KillCommandResponse, LeaseError, PollCommandResponse, RunCommandRequest, RunCommandResponse,
    StdinMode, UnixSignal, WriteStdinRequest, WriteStdinResponse,
};
use trogon_sandbox_protocol::StdinAdmit;
use uuid::Uuid;

use crate::command_builder::{build_command, stdin_mode};
use crate::execution::{Execution, ExecutionState};
use crate::launch::Launch;
use crate::stdin_apply_error::StdinApplyError;
use crate::stdin_handle::StdinHandle;
use crate::supervise::{Supervised, supervise};
use crate::target_user::TargetUser;
use crate::{ExecFrame, LeaseBinding};

/// Frames buffered between the task that owns a child and the HTTP response reading them.
const FRAME_BUFFER: usize = 64;

/// How many signals may wait for the task that owns a child.
const KILL_QUEUE_DEPTH: usize = 8;

/// Commands started inside this actor, keyed by execution id.
///
/// The registry is per-process and therefore per-actor: an actor that crashes and is rebuilt legitimately forgets its
/// executions, which the poll contract already models as `NotFound`.
#[derive(Debug, Default)]
pub struct ExecRegistry {
    executions: Mutex<HashMap<ExecutionId, Arc<Execution>>>,
}

impl ExecRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs a command, returning the frames of the newline-delimited response body.
    ///
    /// A detached request emits a single `Started` frame and leaves the command running; an attached request emits any
    /// streamed chunks followed by exactly one `Completed` frame. Either way the stream is finite and ends the HTTP
    /// response.
    pub async fn run(&self, request: RunCommandRequest, binding: LeaseBinding) -> mpsc::Receiver<ExecFrame> {
        let (frames, receiver) = mpsc::channel(FRAME_BUFFER);

        // Resolved before the spawn so a user that does not exist, or an agent that cannot become it, is a verdict
        // rather than an opaque "failed to start command".
        let command = match prepare(&request, &binding) {
            Ok(command) => command,
            Err(denied) => {
                let _ = frames
                    .send(ExecFrame::Response(RunCommandResponse::Denied(denied)))
                    .await;
                return receiver;
            }
        };

        let execution_id = ExecutionId::new(Uuid::now_v7().to_string());
        let (kill_tx, kill) = mpsc::channel(KILL_QUEUE_DEPTH);
        let (stdin, stdin_ops) = match stdin_mode(&request) {
            StdinMode::Closed => (None, None),
            StdinMode::Interactive => {
                let (handle, ops) = StdinHandle::channel();
                (Some(handle), Some(ops))
            }
        };
        let execution = Arc::new(Execution {
            state: Mutex::new(ExecutionState::Running),
            kill: kill_tx,
            stdin,
        });
        self.executions
            .lock()
            .await
            .insert(execution_id.clone(), Arc::clone(&execution));

        tokio::spawn(supervise(Supervised {
            execution_id,
            execution,
            command,
            request,
            frames,
            kill,
            stdin_ops,
        }));
        receiver
    }

    pub async fn poll(&self, execution_id: &ExecutionId) -> PollCommandResponse {
        let Some(execution) = self.find(execution_id).await else {
            return PollCommandResponse::NotFound;
        };
        let state = execution.state.lock().await;
        match &*state {
            ExecutionState::Running => PollCommandResponse::Running,
            ExecutionState::Completed(output) => PollCommandResponse::Completed(output.clone()),
        }
    }

    pub async fn kill(&self, execution_id: &ExecutionId, signal: UnixSignal) -> KillCommandResponse {
        let Some(execution) = self.find(execution_id).await else {
            return KillCommandResponse::NotFound;
        };
        let state = execution.state.lock().await;
        if matches!(&*state, ExecutionState::Completed(_)) {
            return KillCommandResponse::AlreadyCompleted;
        }
        match execution.kill.send(signal).await {
            Ok(()) => KillCommandResponse::Signaled,
            Err(_) => KillCommandResponse::AlreadyCompleted,
        }
    }

    pub async fn write_stdin(&self, request: &WriteStdinRequest) -> WriteStdinResponse {
        let Some(execution) = self.find(request.execution_id()).await else {
            return WriteStdinResponse::NotFound;
        };
        if matches!(&*execution.state.lock().await, ExecutionState::Completed(_)) {
            return WriteStdinResponse::AlreadyCompleted;
        }
        let Some(handle) = execution.stdin.as_ref() else {
            return WriteStdinResponse::StdinNotInteractive;
        };

        let mut sequencer = handle.sequencer.lock().await;
        if handle.is_broken() {
            return WriteStdinResponse::StdinClosed;
        }
        match sequencer.admit(request.seq(), request.data().clone(), request.eof()) {
            StdinAdmit::Duplicate => WriteStdinResponse::Duplicate {
                next_seq: sequencer.next_seq(),
            },
            StdinAdmit::Buffered => WriteStdinResponse::Buffered {
                next_seq: sequencer.next_seq(),
            },
            StdinAdmit::OutOfWindow => WriteStdinResponse::OutOfWindow {
                next_seq: sequencer.next_seq(),
            },
            StdinAdmit::Closed => WriteStdinResponse::StdinClosed,
            StdinAdmit::Apply(writes) => {
                let mut applied = 0;
                let mut failure = None;
                for write in writes {
                    match handle.apply(write).await {
                        Ok(()) => applied += 1,
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
                sequencer.commit(applied);
                match failure {
                    None => WriteStdinResponse::Written {
                        next_seq: sequencer.next_seq(),
                    },
                    Some(StdinApplyError::Backpressure) => WriteStdinResponse::Backpressure {
                        next_seq: sequencer.next_seq(),
                    },
                    Some(StdinApplyError::Broken) => WriteStdinResponse::StdinClosed,
                }
            }
        }
    }

    async fn find(&self, execution_id: &ExecutionId) -> Option<Arc<Execution>> {
        self.executions.lock().await.get(execution_id).cloned()
    }
}

fn prepare(request: &RunCommandRequest, binding: &LeaseBinding) -> Result<tokio::process::Command, LeaseError> {
    let launch = Launch::parse(request)?;
    let user = request.run_as_user.as_ref().map(TargetUser::resolve).transpose()?;
    Ok(build_command(launch, request, binding, user.as_ref()))
}

#[cfg(test)]
mod tests;

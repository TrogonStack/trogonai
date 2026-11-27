use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use trogon_sandbox_contract::{LeaseId, StdinChunk, StdinEof, StdinMode, StdinSeq, WriteStdinResponse};
use trogon_sandbox_providers::PodExecOutput;

use crate::command_execution_state::CommandExecutionState;
use crate::execution_status::ExecutionStatus;
use crate::execution_task::ExecutionTask;
use crate::tracked_execution::TrackedExecution;
use crate::{K8sWorkerError, PollOutcome, StdinHandle, run_stdin_writer};

/// A command's eventual output, owned by whoever must observe its exit.
pub(crate) type Completion = Pin<Box<dyn Future<Output = Result<PodExecOutput, K8sWorkerError>> + Send>>;

/// An exec whose stdin the tracker owns for the execution's lifetime.
pub(crate) struct AttachedExec {
    pub(crate) stdin: Pin<Box<dyn AsyncWrite + Send>>,
    pub(crate) completion: Completion,
}

/// Tracks detached command executions by generated `execution_id`, decoupled
/// from `K8sMicroVmProvider`/NATS/OCI so the state machine (start -> poll ->
/// completed, repeat-poll idempotency, unknown-id handling, stdin delivery)
/// can be unit tested with a controllable future instead of a real k8s exec.
#[derive(Clone, Default)]
pub(crate) struct ExecutionTracker {
    executions: Arc<Mutex<HashMap<String, TrackedExecution>>>,
}

impl ExecutionTracker {
    /// Records a new `Running` entry and spawns `run_exec` in the background,
    /// updating the entry to `Completed`/`Failed` once it resolves. Returns
    /// the `execution_id` immediately, without awaiting `run_exec`.
    pub(crate) async fn spawn<F, Fut>(&self, lease_id: LeaseId, execution_id: String, run_exec: F) -> String
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<PodExecOutput, K8sWorkerError>> + Send,
    {
        let execution_id = self.insert_running(lease_id, execution_id, None).await;
        self.finish_when_done(execution_id.clone(), async move { run_exec().await })
            .await;
        execution_id
    }

    /// Like [`Self::spawn`], but `start` yields an exec whose stdin the
    /// tracker owns for the execution's lifetime. With `StdinMode::Interactive`
    /// later [`Self::write_stdin`] requests feed it; with `StdinMode::Closed`
    /// it is closed right away.
    pub(crate) async fn spawn_attached<F, Fut>(
        &self,
        lease_id: LeaseId,
        execution_id: String,
        stdin: StdinMode,
        start: F,
    ) -> String
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<AttachedExec, K8sWorkerError>> + Send,
    {
        let (handle, receiver) = match stdin {
            StdinMode::Closed => (None, None),
            StdinMode::Interactive => {
                let (handle, receiver) = StdinHandle::channel();
                (Some(handle), Some(receiver))
            }
        };
        let execution_id = self.insert_running(lease_id, execution_id, handle).await;
        let tracker = self.clone();
        let writer_id = execution_id.clone();
        self.finish_when_done(execution_id.clone(), async move {
            let exec = start().await?;
            match receiver {
                Some(ops) => {
                    tracker.spawn_task(&writer_id, run_stdin_writer(exec.stdin, ops)).await;
                }
                None => {
                    let mut input = exec.stdin;
                    let _ = input.shutdown().await;
                }
            }
            exec.completion.await
        })
        .await;
        execution_id
    }

    async fn insert_running(&self, lease_id: LeaseId, execution_id: String, stdin: Option<StdinHandle>) -> String {
        self.executions.lock().await.insert(
            execution_id.clone(),
            TrackedExecution {
                lease_id,
                state: CommandExecutionState::Running,
                stdin,
                tasks: Vec::new(),
            },
        );
        execution_id
    }

    async fn finish_when_done<Fut>(&self, execution_id: String, run: Fut)
    where
        Fut: Future<Output = Result<PodExecOutput, K8sWorkerError>> + Send + 'static,
    {
        let executions = Arc::clone(&self.executions);
        let id = execution_id.clone();
        self.spawn_task(&execution_id, async move {
            let state = match run.await {
                Ok(output) => CommandExecutionState::Completed(output),
                Err(e) => CommandExecutionState::Failed(e.to_string()),
            };
            if let Some(tracked) = executions.lock().await.get_mut(&id)
                && !matches!(
                    (&tracked.state, &state),
                    (CommandExecutionState::Uncertain(_), CommandExecutionState::Failed(_))
                )
            {
                tracked.state = state;
                tracked.stdin = None;
            }
        })
        .await;
    }

    pub(crate) async fn spawn_task(&self, execution_id: &str, future: impl Future<Output = ()> + Send + 'static) {
        let mut executions = self.executions.lock().await;
        if let Some(execution) = executions.get_mut(execution_id) {
            execution.tasks.push(ExecutionTask::new(tokio::spawn(future)));
        }
    }

    /// Marks a still-tracked execution as having an unconfirmed outcome.
    /// Answers whether the execution was found.
    pub(crate) async fn mark_uncertain(&self, execution_id: &str, message: String) -> bool {
        match self.executions.lock().await.get_mut(execution_id) {
            Some(execution) => {
                execution.state = CommandExecutionState::Uncertain(message);
                true
            }
            None => false,
        }
    }

    pub(crate) async fn dispose_lease(&self, lease_id: &LeaseId) {
        let tasks = {
            let mut executions = self.executions.lock().await;
            let mut tasks = Vec::new();
            executions.retain(|_, execution| {
                if &execution.lease_id == lease_id {
                    tasks.append(&mut execution.tasks);
                    false
                } else {
                    true
                }
            });
            tasks
        };
        for task in tasks {
            task.dispose().await;
        }
    }

    /// Whether `execution_id` is running, finished or unknown to `lease_id`.
    /// An execution owned by another lease reads as unknown.
    pub(crate) async fn status(&self, lease_id: &LeaseId, execution_id: &str) -> ExecutionStatus {
        match self.executions.lock().await.get(execution_id) {
            Some(tracked) if &tracked.lease_id != lease_id => ExecutionStatus::Unknown,
            None => ExecutionStatus::Unknown,
            Some(TrackedExecution {
                state: CommandExecutionState::Running | CommandExecutionState::Uncertain(_),
                ..
            }) => ExecutionStatus::Running,
            Some(_) => ExecutionStatus::Finished,
        }
    }

    /// Looks up the current state of `execution_id`. Completed/failed entries
    /// are kept so repeat polls stay idempotent; an unknown `execution_id`
    /// yields `NotFound`.
    pub(crate) async fn poll(&self, execution_id: &str) -> PollOutcome {
        match self.executions.lock().await.get(execution_id) {
            Some(TrackedExecution {
                state: CommandExecutionState::Running,
                ..
            }) => PollOutcome::Running,
            Some(TrackedExecution {
                state: CommandExecutionState::Completed(output),
                ..
            }) => PollOutcome::Completed(output.clone()),
            Some(TrackedExecution {
                state: CommandExecutionState::Failed(message) | CommandExecutionState::Uncertain(message),
                ..
            }) => PollOutcome::Failed(message.clone()),
            None => PollOutcome::NotFound,
        }
    }

    /// Delivers one stdin write to an interactive execution owned by
    /// `lease_id`; a lease that does not own it sees the same answer as for
    /// an unknown execution.
    pub(crate) async fn write_stdin(
        &self,
        lease_id: &LeaseId,
        execution_id: &str,
        seq: StdinSeq,
        data: StdinChunk,
        eof: StdinEof,
    ) -> WriteStdinResponse {
        let handle = match self.executions.lock().await.get(execution_id) {
            Some(tracked) if &tracked.lease_id != lease_id => {
                return WriteStdinResponse::NotFound;
            }
            None => return WriteStdinResponse::NotFound,
            Some(TrackedExecution {
                state: CommandExecutionState::Uncertain(_),
                ..
            }) => {
                return WriteStdinResponse::StdinClosed;
            }
            Some(TrackedExecution {
                state: CommandExecutionState::Running,
                stdin: Some(handle),
                ..
            }) => handle.clone(),
            Some(TrackedExecution {
                state: CommandExecutionState::Running,
                stdin: None,
                ..
            }) => return WriteStdinResponse::StdinNotInteractive,
            Some(_) => return WriteStdinResponse::AlreadyCompleted,
        };
        handle.deliver(seq, data, eof).await
    }
}

#[cfg(test)]
mod tests;

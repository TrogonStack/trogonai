use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::task::{AbortHandle, JoinHandle};

use super::*;

pub(super) struct CommandTask {
    abort: AbortHandle,
    joined: Shared<BoxFuture<'static, ()>>,
    abort_on_drop: bool,
}

impl Drop for CommandTask {
    fn drop(&mut self) {
        if self.abort_on_drop {
            self.abort.abort();
        }
    }
}

impl CommandTask {
    pub fn new(handle: JoinHandle<()>) -> Self {
        Self::with_drop_policy(handle, false)
    }

    pub fn new_stdin(handle: JoinHandle<()>) -> Self {
        Self::with_drop_policy(handle, true)
    }

    fn with_drop_policy(handle: JoinHandle<()>, abort_on_drop: bool) -> Self {
        Self {
            abort: handle.abort_handle(),
            joined: async move {
                if let Err(error) = handle.await
                    && !error.is_cancelled()
                {
                    warn!(%error, "Docker command collector panicked");
                }
            }
            .boxed()
            .shared(),
            abort_on_drop,
        }
    }

    #[cfg(test)]
    pub fn is_finished(&self) -> bool {
        self.abort.is_finished()
    }

    pub async fn dispose(&self) {
        self.abort.abort();
        self.join().await;
    }

    pub async fn join(&self) {
        self.joined.clone().await;
    }
}

impl DockerWorkerProvider {
    pub(super) async fn register_command_task(
        &self,
        execution_id: &ExecutionId,
        handle: JoinHandle<()>,
    ) -> Arc<CommandTask> {
        let task = Arc::new(CommandTask::new(handle));
        let missing = {
            let mut executions = self.executions.lock().await;
            match executions.get_mut(execution_id) {
                Some(ExecutionState::Running { task: owned, .. }) => {
                    *owned = Some(task.clone());
                    false
                }
                Some(ExecutionState::Completed { .. }) => false,
                None => true,
            }
        };
        if missing {
            task.dispose().await;
        }
        task
    }

    pub(super) fn timeout_output(timeout_seconds: u32) -> CommandOutput {
        CommandOutput {
            exit_code: -1,
            stderr: format!("command timed out after {timeout_seconds}s").into_bytes(),
            timed_out: CommandTimeoutStatus::TimedOut,
            ..Default::default()
        }
    }

    pub(super) async fn record_uncertainty(
        executions: &Mutex<HashMap<ExecutionId, ExecutionState>>,
        execution_id: &ExecutionId,
        message: String,
    ) {
        if let Some(ExecutionState::Running { uncertainty, .. }) = executions.lock().await.get_mut(execution_id) {
            *uncertainty = Some(message);
        }
    }

    pub(super) async fn record_command_result(
        executions: &Mutex<HashMap<ExecutionId, ExecutionState>>,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        output: &anyhow::Result<CommandOutput>,
    ) {
        let mut executions = executions.lock().await;
        let Some(state @ ExecutionState::Running { .. }) = executions.get_mut(execution_id) else {
            return;
        };
        match output {
            Ok(output) => {
                *state = ExecutionState::Completed {
                    lease_id: lease_id.clone(),
                    output: output.clone(),
                };
            }
            Err(error) => {
                if let ExecutionState::Running { uncertainty, .. } = state {
                    *uncertainty = Some(error.to_string());
                }
            }
        }
    }
}

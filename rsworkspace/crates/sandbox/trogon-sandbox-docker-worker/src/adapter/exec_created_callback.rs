use super::*;

/// Makes an exec available for control only after Docker confirms it started.
pub(super) struct ExecCreatedCallback {
    pub(super) executions: Arc<Mutex<HashMap<ExecutionId, ExecutionState>>>,
    pub(super) execution_id: ExecutionId,
}

impl ExecCreatedCallback {
    pub(super) async fn spawn_stdin_task(
        &self,
        input: std::pin::Pin<Box<dyn tokio::io::AsyncWrite + Send>>,
        ops: mpsc::Receiver<StdinOp>,
    ) -> Option<Arc<CommandTask>> {
        let mut executions = self.executions.lock().await;
        if let Some(ExecutionState::Running { stdin_task, .. }) = executions.get_mut(&self.execution_id) {
            let task = Arc::new(CommandTask::new_stdin(tokio::spawn(run_stdin_writer(input, ops))));
            *stdin_task = Some(task.clone());
            Some(task)
        } else {
            None
        }
    }

    pub(super) async fn record(&self, exec_id: String) {
        if let Some(ExecutionState::Running { exec_id: recorded, .. }) =
            self.executions.lock().await.get_mut(&self.execution_id)
        {
            *recorded = Some(exec_id);
        }
    }
}

use std::collections::BTreeMap;

use trogon_sandbox_contract::{
    CommandOutput, CommandTimeoutStatus, ExecutionId, KillCommandRequest, KillCommandResponse, LeaseId,
    OutputTruncation, PollCommandResponse, ProcessState,
};

use super::constants::KILL_SCRIPT;
use super::e2b_exec::E2bExec;
use super::failure::after_launch;
use super::terminal::Terminal;
use crate::LeaseSandbox;

impl E2bExec {
    pub async fn handle_poll_command(
        &self,
        sandbox: &LeaseSandbox,
        lease_id: &str,
        execution_id: &str,
    ) -> PollCommandResponse {
        let lease = LeaseId::new(lease_id);
        let _gate = self.freeze_executions(lease_id).await;
        self.poll_execution(sandbox, &lease, &ExecutionId::new(execution_id))
            .await
    }

    pub(super) async fn poll_execution(
        &self,
        sandbox: &LeaseSandbox,
        lease: &LeaseId,
        execution_id: &ExecutionId,
    ) -> PollCommandResponse {
        let Some(execution) = self.execution(lease, execution_id).await else {
            return PollCommandResponse::NotFound;
        };
        let terminal = match execution.deadline.after_expiry(&self.envd, sandbox, &execution).await {
            Ok(terminal) => terminal,
            Err(error) => return PollCommandResponse::Denied(after_launch(error)),
        };
        let timed_out = matches!(terminal, Some(Terminal::TimedOut));
        let exit_code = match terminal {
            Some(Terminal::TimedOut) => CommandOutput::TIMED_OUT_EXIT_CODE,
            Some(Terminal::Natural(code)) => code,
            Some(Terminal::Pending | Terminal::Uncertain(_)) | None => {
                match self.exit_status(sandbox, &execution).await {
                    Ok(Some(code)) => match self.process_state(sandbox, &execution).await {
                        Ok(ProcessState::Exited { .. }) => code,
                        Ok(ProcessState::Running) => return PollCommandResponse::Running,
                        Err(error) => return PollCommandResponse::Denied(after_launch(error)),
                    },
                    Ok(None) => return PollCommandResponse::Running,
                    Err(error) => return PollCommandResponse::Denied(error),
                }
            }
        };
        let (stdout, out_truncated) = match self.side_file(sandbox, &execution, "out").await {
            Ok(output) => output,
            Err(error) => return PollCommandResponse::Denied(error),
        };
        let (stderr, err_truncated) = match self.side_file(sandbox, &execution, "err").await {
            Ok(output) => output,
            Err(error) => return PollCommandResponse::Denied(error),
        };
        PollCommandResponse::Completed(CommandOutput {
            exit_code,
            stdout,
            stderr,
            truncated: OutputTruncation::from(out_truncated || err_truncated),
            timed_out: CommandTimeoutStatus::from(timed_out),
        })
    }

    pub async fn handle_kill_command(
        &self,
        sandbox: &LeaseSandbox,
        lease_id: &str,
        request: &KillCommandRequest,
    ) -> KillCommandResponse {
        let lease = LeaseId::new(lease_id);
        let _gate = self.freeze_executions(lease_id).await;
        let Some(execution) = self.execution(&lease, &request.execution_id).await else {
            return KillCommandResponse::NotFound;
        };
        match self.exit_status(sandbox, &execution).await {
            Ok(Some(_)) => match self.process_state(sandbox, &execution).await {
                Ok(ProcessState::Exited { .. }) => return KillCommandResponse::AlreadyCompleted,
                Ok(ProcessState::Running) => {}
                Err(error) => return KillCommandResponse::Denied(after_launch(error)),
            },
            Ok(None) => {}
            Err(error) => return KillCommandResponse::Denied(error),
        }
        let args = vec![
            execution.dir.clone(),
            request.signal.as_str().to_string(),
            execution.identity.pid.to_string(),
            execution.identity.started.to_string(),
        ];
        match self
            .run_script(sandbox, &execution.user, KILL_SCRIPT, &args, &BTreeMap::new(), None)
            .await
        {
            Ok(()) => KillCommandResponse::Signaled,
            Err(error) => match self.process_state(sandbox, &execution).await {
                Ok(ProcessState::Exited { .. }) => KillCommandResponse::AlreadyCompleted,
                _ => KillCommandResponse::Denied(error),
            },
        }
    }
}

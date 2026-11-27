use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};
use trogon_sandbox_contract::{
    ExecutionId, LeaseError, LeaseId, ListProcessesResponse, ProcessInfo, ProcessState, SandboxUserName,
};

use super::constants::{PROCESS_ABSENT_EXIT_CODE, PROCESS_IDENTITY};
use super::deadline::Deadline;
use super::e2b_exec::E2bExec;
use super::execution::Execution;
use super::failure::invalid_spec;
use crate::LeaseSandbox;
use crate::snapshots::{CapturedExecution, MAX_EXECUTIONS, ProcessIdentity};

impl E2bExec {
    /// Forgets every detached command of a lease; called when the lease is released.
    pub async fn forget_lease(&self, lease_id: &str) {
        self.watches.forget_lease(lease_id);
        self.tracking.forget(lease_id);
        let lease = LeaseId::new(lease_id);
        let removed = {
            let mut executions = self.executions.lock().await;
            let keys = executions
                .keys()
                .filter(|(held, _)| *held == lease)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| executions.remove(&key))
                .collect::<Vec<_>>()
        };
        for execution in removed {
            execution.deadline.stop_supervisor().await;
        }
        self.execution_gates.lock().await.remove(&lease);
    }

    /// Holds every command of a lease still while the guard lives, so a snapshot sees a settled inventory.
    pub(crate) async fn freeze_executions(&self, lease_id: &str) -> OwnedMutexGuard<()> {
        self.execution_gate(&LeaseId::new(lease_id)).await.lock_owned().await
    }

    pub(super) async fn execution_gate(&self, lease: &LeaseId) -> Arc<Mutex<()>> {
        self.execution_gates
            .lock()
            .await
            .entry(lease.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub(crate) async fn capture_executions(&self, lease_id: &str) -> Result<Vec<CapturedExecution>, LeaseError> {
        let lease = LeaseId::new(lease_id);
        let executions = self.executions.lock().await;
        let mut captured = Vec::new();
        for ((held, execution_id), execution) in executions.iter() {
            if *held != lease {
                continue;
            }
            if captured.len() == MAX_EXECUTIONS {
                return Err(invalid_spec("snapshot execution inventory exceeds its limit"));
            }
            captured.push(CapturedExecution {
                execution_id: execution_id.to_string(),
                user: SandboxUserName::new(execution.user.clone()).map_err(|error| invalid_spec(error.to_string()))?,
                identity: execution.identity,
                max_output_bytes: execution.max_output_bytes as u64,
                deadline: Some(execution.deadline.capture().await?),
            });
        }
        captured.sort_by(|a, b| a.execution_id.cmp(&b.execution_id));
        Ok(captured)
    }

    pub(super) async fn process_state(
        &self,
        sandbox: &LeaseSandbox,
        execution: &Execution,
    ) -> Result<ProcessState, LeaseError> {
        let output = self
            .script_result(
                sandbox,
                &execution.user,
                PROCESS_IDENTITY,
                &[execution.identity.pid.to_string()],
                &BTreeMap::new(),
                None,
            )
            .await?;
        if output.exit_code == 0 {
            let actual = ProcessIdentity::parse(&output.stdout).map_err(|error| invalid_spec(error.to_string()))?;
            if actual != execution.identity {
                return Err(invalid_spec("process start identity does not match its host capture"));
            }
            return Ok(ProcessState::Running);
        }
        if output.exit_code == PROCESS_ABSENT_EXIT_CODE
            && let Some(exit_code) = self.exit_status(sandbox, execution).await?
        {
            return Ok(ProcessState::Exited { exit_code });
        }
        Err(invalid_spec(
            "captured process identity is unavailable; refusing ambiguous adoption",
        ))
    }

    /// Adopts the commands a snapshot captured, once each one's native process is confirmed.
    pub(crate) async fn restore_executions(
        &self,
        sandbox: &LeaseSandbox,
        lease_id: &str,
        captured: &[CapturedExecution],
    ) -> Result<(), LeaseError> {
        let lease = LeaseId::new(lease_id);
        let mut restored = Vec::new();
        for entry in captured {
            let mut execution = Execution {
                lease_id: lease.clone(),
                dir: entry.directory(),
                user: entry.user.as_str().into(),
                identity: entry.identity,
                max_output_bytes: usize::try_from(entry.max_output_bytes)
                    .map_err(|_| invalid_spec("captured output limit exceeds this worker"))?,
                deadline: Deadline::Lease,
            };
            let state = self.process_state(sandbox, &execution).await?;
            execution.deadline = Deadline::restore(entry.deadline.as_ref(), state)?;
            execution.deadline.arm(&self.envd, sandbox, &execution).await?;
            execution.deadline.after_expiry(&self.envd, sandbox, &execution).await?;
            restored.push(((lease.clone(), ExecutionId::new(entry.execution_id.clone())), execution));
        }
        let mut executions = self.executions.lock().await;
        if executions.keys().any(|(held, _)| *held == lease) {
            return Err(invalid_spec("restored lease already has execution identities"));
        }
        let gate = self.execution_gate(&lease).await;
        for (_, execution) in &mut restored {
            let mut deadline = execution.deadline.clone();
            deadline.supervise(self.envd.clone(), sandbox.clone(), execution, gate.clone());
            execution.deadline = deadline;
        }
        executions.extend(restored);
        Ok(())
    }

    pub async fn handle_list_processes(&self, sandbox: &LeaseSandbox, lease_id: &str) -> ListProcessesResponse {
        let lease = LeaseId::new(lease_id);
        let mut entries: Vec<_> = self
            .executions
            .lock()
            .await
            .iter()
            .filter(|((held, _), _)| *held == lease)
            .take(MAX_EXECUTIONS + 1)
            .map(|((_, id), execution)| (id.clone(), execution.clone()))
            .collect();
        if entries.len() > MAX_EXECUTIONS {
            return ListProcessesResponse::Denied(invalid_spec("execution inventory exceeds its limit"));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut processes = Vec::with_capacity(entries.len());
        for (execution_id, execution) in entries {
            let state = match self.process_state(sandbox, &execution).await {
                Ok(state) => state,
                Err(error) => return ListProcessesResponse::Denied(error),
            };
            processes.push(ProcessInfo {
                execution_id,
                name: None,
                readiness: None,
                state,
            });
        }
        ListProcessesResponse::Processes { processes }
    }

    pub(super) async fn execution(&self, lease: &LeaseId, execution_id: &ExecutionId) -> Option<Execution> {
        self.executions
            .lock()
            .await
            .get(&(lease.clone(), execution_id.clone()))
            .filter(|execution| execution.lease_id == *lease)
            .cloned()
    }

    pub(super) async fn remember_execution(
        &self,
        sandbox: &LeaseSandbox,
        lease: &LeaseId,
        execution_id: &ExecutionId,
        mut execution: Execution,
    ) {
        let mut deadline = execution.deadline.clone();
        deadline.supervise(
            self.envd.clone(),
            sandbox.clone(),
            &execution,
            self.execution_gate(lease).await,
        );
        execution.deadline = deadline;
        self.executions
            .lock()
            .await
            .insert((lease.clone(), execution_id.clone()), execution);
    }
}

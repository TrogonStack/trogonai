use std::time::Duration;

use trogon_sandbox_contract::{
    ExecutionId, ListProcessesResponse, ProcessInfo, ProcessState, ReadinessProbe, WaitReadyRequest, WaitReadyResponse,
};

use super::exec_state::ExecState;
use super::lease::Lease;
use crate::live_output::contains_bytes;
use crate::mutex_ext::MutexExt;

/// The named and probed processes of the lease, by execution id.
pub(crate) fn list_processes(lease: &Lease) -> ListProcessesResponse {
    let procs = lease.procs.lock_clean();
    let mut processes: Vec<ProcessInfo> = procs
        .iter()
        .filter_map(|(id, proc)| {
            let exit = lease.exit_code(id)?;
            Some(ProcessInfo {
                execution_id: id.clone(),
                name: proc.name.clone(),
                readiness: proc.readiness.clone(),
                state: match exit {
                    None => ProcessState::Running,
                    Some(exit_code) => ProcessState::Exited { exit_code },
                },
            })
        })
        .collect();
    processes.sort_by(|a, b| a.execution_id.cmp(&b.execution_id));
    ListProcessesResponse::Processes { processes }
}

/// Polls the probe until it passes, the process exits, or the timeout lapses.
/// A probe that passed before the process exited still counts as ready.
pub(crate) async fn wait_ready(lease: &Lease, request: WaitReadyRequest) -> WaitReadyResponse {
    let (probe, live) = {
        let procs = lease.procs.lock_clean();
        match procs.get(&request.execution_id) {
            None => return WaitReadyResponse::NotFound,
            Some(proc) => match &proc.readiness {
                None => return WaitReadyResponse::NoProbe,
                Some(probe) => (probe.clone(), proc.live.clone()),
            },
        }
    };
    let timeout = request.timeout_seconds.min(WaitReadyRequest::MAX_TIMEOUT_SECONDS);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(u64::from(timeout));
    loop {
        let exit = lease.exit_code(&request.execution_id);
        let ready = match &probe {
            ReadinessProbe::OutputContains { needle } => {
                live.contains(needle.as_str().as_bytes())
                    || matches!(&exit, Some(Some(_))) && final_output_has(lease, &request.execution_id, needle.as_str())
            }
            ReadinessProbe::TcpPort { port } => exit == Some(None) && live.is_listening(port.get()),
        };
        if ready {
            return WaitReadyResponse::Ready;
        }
        match exit {
            None => return WaitReadyResponse::NotFound,
            Some(Some(exit_code)) => return WaitReadyResponse::Failed { exit_code },
            Some(None) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return WaitReadyResponse::NotReady;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Whether the final output of a finished execution contains `needle`.
fn final_output_has(lease: &Lease, execution_id: &ExecutionId, needle: &str) -> bool {
    matches!(
        lease.execs.lock_clean().get(execution_id),
        Some(ExecState::Done(output))
            if [&output.stdout, &output.stderr]
                .iter()
                .any(|stream| contains_bytes(stream, needle.as_bytes()))
    )
}

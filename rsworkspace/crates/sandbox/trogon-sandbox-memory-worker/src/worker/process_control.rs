use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{
    ExecutionId, KillCommandResponse, RestartCommandResponse, RunCommandResponse, StopCommandResponse, StopSignal,
    UnixSignal,
};

use super::command_launch::{denied, killed_output, run_command};
use super::exec_state::ExecState;
use super::lease::Lease;
use super::state::State;
use crate::mutex_ext::MutexExt;

/// Marks `execution_id` killed unless it already finished.
fn settle_killed(lease: &Lease, execution_id: &ExecutionId) {
    let mut execs = lease.execs.lock_clean();
    if matches!(execs.get(execution_id), Some(ExecState::Running { .. })) {
        execs.insert(execution_id.clone(), ExecState::Done(killed_output()));
    }
}

pub(crate) async fn kill_command(lease: &Lease, execution_id: &ExecutionId, signal: UnixSignal) -> KillCommandResponse {
    let task = {
        let execs = lease.execs.lock_clean();
        match execs.get(execution_id) {
            None => return KillCommandResponse::NotFound,
            Some(ExecState::Done(_)) => return KillCommandResponse::AlreadyCompleted,
            Some(ExecState::Running { task }) => task.clone(),
        }
    };
    if signal == UnixSignal::Kill {
        task.terminate();
        task.join().await;
        settle_killed(lease, execution_id);
    } else {
        *task.signal.lock_clean() = Some(signal);
        task.cancel.cancel();
    }
    KillCommandResponse::Signaled
}

/// Waits up to `limit` for `execution_id` to finish.
async fn wait_done(lease: &Lease, execution_id: &ExecutionId, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if !matches!(lease.exit_code(execution_id), Some(None)) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// SIGTERM, then SIGKILL when the command is still running after `grace`.
/// A command that honours its cancellation token ends on the SIGTERM.
pub(crate) async fn stop_command(lease: &Lease, execution_id: &ExecutionId, grace: Duration) -> StopCommandResponse {
    let Some(state) = lease.exit_code(execution_id) else {
        return StopCommandResponse::NotFound;
    };
    let task = lease.tasks.lock_clean().get(execution_id.as_str()).cloned();
    let Some(task) = task else {
        return StopCommandResponse::NotFound;
    };
    if state.is_some() {
        task.join().await;
        return StopCommandResponse::AlreadyCompleted;
    }
    *task.signal.lock_clean() = Some(UnixSignal::Term);
    task.cancel.cancel();
    if wait_done(lease, execution_id, grace).await {
        task.join().await;
        return StopCommandResponse::Stopped {
            signal: StopSignal::Term,
        };
    }
    task.terminate();
    task.join().await;
    settle_killed(lease, execution_id);
    StopCommandResponse::Stopped {
        signal: StopSignal::Kill,
    }
}

/// Runs the command `execution_id` started with again, under a new id. A
/// command still running is stopped first.
pub(crate) async fn restart_command(
    state: &State,
    lease: &Arc<Lease>,
    execution_id: &ExecutionId,
    grace: Duration,
) -> RestartCommandResponse {
    let Some(launch) = lease.launches.lock_clean().get(execution_id).cloned() else {
        return RestartCommandResponse::NotFound;
    };
    stop_command(lease, execution_id, grace).await;
    match run_command(state, lease, launch).await {
        RunCommandResponse::Started(started) => RestartCommandResponse::Restarted {
            execution_id: started.execution_id,
        },
        RunCommandResponse::Denied(error) => RestartCommandResponse::Denied(error),
        RunCommandResponse::Completed(_) => RestartCommandResponse::Denied(denied("unexpected completed reply")),
    }
}

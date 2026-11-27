use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    CommandDeadline, CommandExecution, CommandLaunchOrigin, CommandOutput, CommandTimeoutStatus, ExecutionId,
    LaunchCancellation, LeaseError, RunCommandRequest, RunCommandResponse, RunCommandStartedResponse, StdinMode,
    UnixSignal,
};
use trogon_sandbox_protocol::RedactionSet;

use super::command_task::{ClosePolicy, CommandTask};
use super::exec_state::ExecState;
use super::lease::Lease;
use super::proc_entry::Proc;
use super::state::State;
use crate::constants::{DEFAULT_CWD, KILLED_EXIT_CODE};
use crate::mutex_ext::MutexExt;
use crate::{CommandCtx, LiveOutput, resolve};

pub(crate) fn denied(message: impl Into<String>) -> LeaseError {
    LeaseError::Backend {
        message: message.into(),
    }
}

pub(crate) fn signal_number(signal: UnixSignal) -> i32 {
    match signal {
        UnixSignal::Hup => 1,
        UnixSignal::Int => 2,
        UnixSignal::Quit => 3,
        UnixSignal::Kill => 9,
        UnixSignal::Usr1 => 10,
        UnixSignal::Usr2 => 12,
        UnixSignal::Term => 15,
    }
}

/// The output of a command killed before it finished.
pub(crate) fn killed_output() -> CommandOutput {
    CommandOutput {
        exit_code: KILLED_EXIT_CODE,
        ..Default::default()
    }
}

/// What the worker cannot honor of a request that parsed.
fn unsupported_feature(request: &RunCommandRequest) -> Option<&'static str> {
    if request.require_launch_cancellation == LaunchCancellation::Required {
        return Some("command launch cancellation is not supported by this provider");
    }
    match &request.execution {
        CommandExecution::Detached {
            stdin: StdinMode::Interactive,
            ..
        } => Some("interactive stdin is not supported by this provider"),
        CommandExecution::Detached {
            deadline: CommandDeadline::Lease,
            ..
        } => Some("lease deadline is not supported by this provider"),
        _ => None,
    }
}

pub(crate) async fn run_command(state: &State, lease: &Arc<Lease>, request: RunCommandRequest) -> RunCommandResponse {
    if let Some(reason) = unsupported_feature(&request) {
        return RunCommandResponse::Denied(denied(reason));
    }
    let admission = lease.admission.lock().await;
    if lease.closing.load(Ordering::SeqCst) {
        return RunCommandResponse::Denied(denied("lease is closing"));
    }
    let cancel = CancellationToken::new();
    let signal = Arc::new(Mutex::new(None));
    let live = LiveOutput::default();
    let execution_id = ExecutionId::new(uuid::Uuid::now_v7().to_string());
    let process = match &request.execution {
        CommandExecution::Detached { process, .. } => process.as_ref(),
        CommandExecution::Attached => None,
    };
    if let Some(process) = process {
        let mut procs = lease.procs.lock_clean();
        let existing = process.name.as_ref().and_then(|name| {
            procs
                .iter()
                .find(|(_, proc)| proc.name.as_ref() == Some(name))
                .map(|(id, _)| id.clone())
        });
        if let Some(existing) = existing {
            if matches!(lease.exit_code(&existing), Some(None)) {
                return RunCommandResponse::Started(RunCommandStartedResponse {
                    origin: CommandLaunchOrigin::Reused,
                    execution_id: existing,
                });
            }
            procs.remove(&existing);
        }
        procs.insert(
            execution_id.clone(),
            Proc {
                name: process.name.clone(),
                readiness: process.readiness.clone(),
                live: live.clone(),
            },
        );
    }
    let detached = matches!(request.execution, CommandExecution::Detached { .. });
    if detached {
        lease
            .launches
            .lock_clean()
            .insert(execution_id.clone(), request.clone());
    }
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let (output_tx, output_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn({
        let commands = state.commands.clone();
        let lease = Arc::clone(lease);
        let id = execution_id.clone();
        let cancel = cancel.clone();
        let signal = signal.clone();
        async move {
            let _ = start_rx.await;
            let output = execute(&commands, &lease, &request, cancel, signal, live).await;
            if detached {
                lease.execs.lock_clean().insert(id, ExecState::Done(output.clone()));
            }
            let _ = output_tx.send(output);
        }
    });
    let task = CommandTask::new(handle, cancel, signal, ClosePolicy::Abort);
    lease
        .tasks
        .lock_clean()
        .insert(execution_id.as_str().to_string(), task.clone());
    if detached {
        lease
            .execs
            .lock_clean()
            .insert(execution_id.clone(), ExecState::Running { task: task.clone() });
    }
    let _ = start_tx.send(());
    drop(admission);
    if detached {
        return RunCommandResponse::Started(RunCommandStartedResponse {
            origin: CommandLaunchOrigin::Created,
            execution_id,
        });
    }
    let output = output_rx.await.unwrap_or_else(|_| killed_output());
    task.join().await;
    lease.tasks.lock_clean().remove(execution_id.as_str());
    RunCommandResponse::Completed(output)
}

async fn execute(
    commands: &crate::CommandTable,
    lease: &Lease,
    request: &RunCommandRequest,
    cancel: CancellationToken,
    signal: Arc<Mutex<Option<UnixSignal>>>,
    live: LiveOutput,
) -> CommandOutput {
    // A relative cwd resolves against the default working directory, and a
    // missing one fails the way a shell's `cd` would, before anything runs.
    let cwd = resolve(DEFAULT_CWD, request.cwd.as_deref().unwrap_or(DEFAULT_CWD));
    if !lease.fs.lock_clean().is_dir(&cwd) {
        return CommandOutput {
            exit_code: 1,
            stderr: format!("cd: {cwd}: No such file or directory\n").into_bytes(),
            ..Default::default()
        };
    }
    let ctx = CommandCtx {
        args: Vec::new(),
        cwd,
        env: request.env.as_map().clone(),
        fs: Arc::clone(&lease.fs),
        cancel: cancel.clone(),
        live,
    };
    let run = commands.run(&request.command, ctx);
    let mut output = match tokio::time::timeout(Duration::from_secs(u64::from(request.timeout_seconds)), run).await {
        Ok(output) => output,
        Err(_) => {
            cancel.cancel();
            return CommandOutput {
                exit_code: CommandOutput::TIMED_OUT_EXIT_CODE,
                stdout: Vec::new(),
                stderr: b"timed out\n".to_vec(),
                timed_out: CommandTimeoutStatus::TimedOut,
                ..Default::default()
            };
        }
    };
    if let Some(signal) = *signal.lock_clean() {
        output.exit_code = 128 + signal_number(signal);
    }
    let redaction = RedactionSet::from_request_values(&request.redact_values);
    output.stdout = redaction.redact(&output.stdout);
    output.stderr = redaction.redact(&output.stderr);
    output
}

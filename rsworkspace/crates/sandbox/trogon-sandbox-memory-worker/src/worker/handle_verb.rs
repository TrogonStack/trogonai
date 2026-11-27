use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use trogon_sandbox_contract::{
    KillCommandRequest, KillCommandResponse, ListProcessesRequest, ListProcessesResponse, NativeHandle,
    NativeHandleResponse, PollCommandRequest, PollCommandResponse, ReadFileRequest, ReadFileResponse,
    RestartCommandRequest, RestartCommandResponse, RunCommandRequest, RunCommandResponse, SandboxFreezeResponse,
    SandboxSuspendRequest, SandboxSuspendResponse, SandboxWakeRequest, SandboxWakeResponse, StopCommandRequest,
    StopCommandResponse, SuspendLevel, WaitReadyRequest, WaitReadyResponse, WriteFilesRequest, WriteFilesResponse,
};

use super::command_launch::{denied, run_command};
use super::exec_state::ExecState;
use super::lease::Lease;
use super::process_control::{kill_command, restart_command, stop_command};
use super::process_query::{list_processes, wait_ready};
use super::state::State;
use super::verb::ServedVerb;
use crate::mutex_ext::MutexExt;

fn bad(error: serde_json::Error) -> trogon_sandbox_contract::LeaseError {
    denied(format!("invalid request: {error}"))
}

fn paused_denial() -> trogon_sandbox_contract::LeaseError {
    denied("lease is paused")
}

/// Serves one modelled verb and returns the JSON reply.
pub(crate) async fn handle_verb(
    state: &State,
    lease: &Arc<Lease>,
    verb: ServedVerb,
    body: &[u8],
) -> serde_json::Result<Vec<u8>> {
    let paused = lease.paused.load(Ordering::SeqCst);
    match verb {
        ServedVerb::NativeHandle => serde_json::to_vec(&NativeHandleResponse::Available(NativeHandle {
            provider: state.kind,
            vendor_id: format!("memory-{}", lease.id),
            url: None,
            region: lease.region.clone(),
        })),
        ServedVerb::Pause | ServedVerb::Resume => {
            // A repeat is a no-op, so both verbs are idempotent.
            lease.paused.store(verb == ServedVerb::Pause, Ordering::SeqCst);
            serde_json::to_vec(&SandboxFreezeResponse::Done)
        }
        // Memory suspend keeps every process, so it freezes the lease the way pause does.
        ServedVerb::Suspend => serde_json::to_vec(&match serde_json::from_slice::<SandboxSuspendRequest>(body) {
            Ok(request) if request.level == SuspendLevel::Memory => {
                lease.paused.store(true, Ordering::SeqCst);
                SandboxSuspendResponse::Done {
                    artifact: None,
                    artifact_bytes: None,
                }
            }
            Ok(_) => SandboxSuspendResponse::Unsupported,
            Err(error) => SandboxSuspendResponse::Denied(bad(error)),
        }),
        ServedVerb::Wake => serde_json::to_vec(&match serde_json::from_slice::<SandboxWakeRequest>(body) {
            Ok(request) if request.level == SuspendLevel::Memory => {
                lease.paused.store(false, Ordering::SeqCst);
                SandboxWakeResponse::Done
            }
            Ok(_) => SandboxWakeResponse::Unsupported,
            Err(error) => SandboxWakeResponse::Denied(bad(error)),
        }),
        ServedVerb::RunCommand if paused => serde_json::to_vec(&RunCommandResponse::Denied(paused_denial())),
        ServedVerb::WriteFiles if paused => serde_json::to_vec(&WriteFilesResponse::Denied(paused_denial())),
        ServedVerb::RestartCommand if paused => serde_json::to_vec(&RestartCommandResponse::Denied(paused_denial())),
        ServedVerb::RunCommand => serde_json::to_vec(&match serde_json::from_slice::<RunCommandRequest>(body) {
            Err(error) => RunCommandResponse::Denied(bad(error)),
            Ok(request) => run_command(state, lease, request).await,
        }),
        ServedVerb::PollCommand => serde_json::to_vec(&match serde_json::from_slice::<PollCommandRequest>(body) {
            Err(error) => PollCommandResponse::Denied(bad(error)),
            Ok(request) => match lease.execs.lock_clean().get(&request.execution_id) {
                None => PollCommandResponse::NotFound,
                Some(ExecState::Running { .. }) => PollCommandResponse::Running,
                Some(ExecState::Done(output)) => PollCommandResponse::Completed(output.clone()),
            },
        }),
        ServedVerb::KillCommand => serde_json::to_vec(&match serde_json::from_slice::<KillCommandRequest>(body) {
            Err(error) => KillCommandResponse::Denied(bad(error)),
            Ok(request) => kill_command(lease, &request.execution_id, request.signal).await,
        }),
        ServedVerb::StopCommand => serde_json::to_vec(&match serde_json::from_slice::<StopCommandRequest>(body) {
            Err(error) => StopCommandResponse::Denied(bad(error)),
            Ok(request) => {
                let grace = request.grace_seconds.min(StopCommandRequest::MAX_GRACE_SECONDS);
                stop_command(lease, &request.execution_id, Duration::from_secs(u64::from(grace))).await
            }
        }),
        ServedVerb::RestartCommand => {
            serde_json::to_vec(&match serde_json::from_slice::<RestartCommandRequest>(body) {
                Err(error) => RestartCommandResponse::Denied(bad(error)),
                Ok(request) => {
                    let grace = request.grace_seconds.min(StopCommandRequest::MAX_GRACE_SECONDS);
                    restart_command(
                        state,
                        lease,
                        &request.execution_id,
                        Duration::from_secs(u64::from(grace)),
                    )
                    .await
                }
            })
        }
        ServedVerb::ListProcesses => serde_json::to_vec(&match serde_json::from_slice::<ListProcessesRequest>(body) {
            Err(error) => ListProcessesResponse::Denied(bad(error)),
            Ok(_) => list_processes(lease),
        }),
        ServedVerb::WaitReady => serde_json::to_vec(&match serde_json::from_slice::<WaitReadyRequest>(body) {
            Err(error) => WaitReadyResponse::Denied(bad(error)),
            Ok(request) => wait_ready(lease, request).await,
        }),
        ServedVerb::WriteFiles => serde_json::to_vec(&match serde_json::from_slice::<WriteFilesRequest>(body) {
            Err(error) => WriteFilesResponse::Denied(bad(error)),
            Ok(request) => {
                let mut fs = lease.fs.lock_clean();
                for file in request.files {
                    fs.write(&file.path, file.content);
                }
                WriteFilesResponse::Written
            }
        }),
        ServedVerb::ReadFile => serde_json::to_vec(&match serde_json::from_slice::<ReadFileRequest>(body) {
            Err(error) => ReadFileResponse::Denied(bad(error)),
            Ok(request) => match lease.fs.lock_clean().read(&request.path) {
                Some(data) => ReadFileResponse::Content { data: data.to_vec() },
                None => ReadFileResponse::NotFound,
            },
        }),
        ServedVerb::FileOp => serde_json::to_vec(&lease.file_op(body).await),
    }
}

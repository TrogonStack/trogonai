use std::path::Path;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, LifecycleEvent, ProviderLease, SandboxId, SandboxPhase, WorkspaceSource,
};
use trogon_sandbox_protocol::subjects::evidence_subject;
use trogon_sandbox_protocol::{LifecycleEmitter, PhaseTimer, run_phase};

use crate::constants::GUEST_KIND_ENV;
use crate::guest_kind::GuestKind;
use crate::persistent_runtime::wants_persistent_runtime;
use crate::v8_wasm_lease_error::V8WasmLeaseError;
use crate::{CapturedOutput, GuestOutcome, JsSession, run_js, run_wasm};

/// Runs a lease's guest to completion, emitting its lifecycle as it goes. A persistent runtime that
/// finished its first script is handed back through `session_out`.
pub(crate) async fn run_lease(
    request: &LeaseRequest,
    lease_id: &LeaseId,
    emitter: &dyn LifecycleEmitter,
    cancel: CancellationToken,
    session_out: &mut Option<JsSession>,
) -> Result<ProviderLease, V8WasmLeaseError> {
    let sandbox_id = SandboxId::new(lease_id.as_str())?;
    emitter
        .emit(LifecycleEvent::Started {
            sandbox_id: sandbox_id.clone(),
        })
        .await;
    let outcome = stages(request, emitter, cancel, session_out).await;
    emitter
        .emit(match &outcome {
            Ok(()) => LifecycleEvent::Completed {
                sandbox_id: sandbox_id.clone(),
            },
            Err(error) => LifecycleEvent::Failed {
                sandbox_id: sandbox_id.clone(),
                error: error.to_string(),
            },
        })
        .await;
    outcome.map(|()| ProviderLease {
        sandbox_id,
        evidence_subject: evidence_subject(lease_id),
        worker_id: None,
        attempt_id: None,
        epoch: None,
    })
}

async fn stages(
    request: &LeaseRequest,
    emitter: &dyn LifecycleEmitter,
    cancel: CancellationToken,
    session_out: &mut Option<JsSession>,
) -> Result<(), V8WasmLeaseError> {
    let path = run_phase(Some(emitter), SandboxPhase::Allocate, None, || {
        std::future::ready(host_mount_path(request))
    })
    .await?;
    let kind = guest_kind(request)?;
    let persistent = wants_persistent_runtime(request);
    if persistent && kind != GuestKind::Js {
        return Err(V8WasmLeaseError::PersistentRuntimeNeedsJs);
    }
    let source_path = Path::new(path).join(match kind {
        GuestKind::Js => "index.js",
        GuestKind::Wasm => "module.wasm",
    });
    let bytes = run_phase(Some(emitter), SandboxPhase::BootRuntime, None, || async {
        tokio::fs::read(&source_path)
            .await
            .map_err(|source| V8WasmLeaseError::ReadSource {
                path: source_path.display().to_string(),
                source,
            })
    })
    .await?;

    let execute = PhaseTimer::start(SandboxPhase::Execute);
    emitter.emit(execute.phase_started()).await;
    let (output, result) = match run_guest(request, kind, persistent, bytes, cancel, session_out).await {
        Ok(outcome) => {
            let (output, result) = outcome.into_result();
            (output, result.map_err(V8WasmLeaseError::Guest))
        }
        Err(error) => (CapturedOutput::default(), Err(error)),
    };
    for (stream, text) in output.lines {
        emitter.emit(LifecycleEvent::Output { stream, text }).await;
    }
    emitter
        .emit(match &result {
            Ok(()) => execute.complete(),
            Err(error) => execute.fail(error.to_string()),
        })
        .await;

    run_phase(Some(emitter), SandboxPhase::Reclaim, None, || {
        std::future::ready(Ok::<(), V8WasmLeaseError>(()))
    })
    .await?;
    result
}

fn host_mount_path(request: &LeaseRequest) -> Result<&str, V8WasmLeaseError> {
    match request.spec.workspace() {
        WorkspaceSource::HostMount { path, .. } => Ok(path.as_str()),
        other => Err(V8WasmLeaseError::UnsupportedWorkspace(format!("{other:?}"))),
    }
}

fn guest_kind(request: &LeaseRequest) -> Result<GuestKind, V8WasmLeaseError> {
    GuestKind::from_image(request.spec.image()).map_or_else(
        || {
            request
                .spec
                .env()
                .get(GUEST_KIND_ENV)
                .ok_or(V8WasmLeaseError::MissingGuestKind)
                .and_then(|value| GuestKind::parse(value))
        },
        Ok,
    )
}

async fn run_guest(
    request: &LeaseRequest,
    kind: GuestKind,
    persistent: bool,
    bytes: Vec<u8>,
    cancel: CancellationToken,
    session_out: &mut Option<JsSession>,
) -> Result<GuestOutcome, V8WasmLeaseError> {
    let timeout = Duration::from_secs(u64::from(request.spec.cpu_seconds().get()));
    let memory_mib = request.spec.memory_mib().get();
    match kind {
        GuestKind::Js => {
            let source = String::from_utf8(bytes)?;
            if persistent {
                run_persistent(source, memory_mib, timeout, &cancel, session_out).await
            } else {
                Ok(run_js(source, memory_mib, timeout, cancel).await?)
            }
        }
        GuestKind::Wasm => Ok(run_wasm(
            bytes,
            memory_mib,
            timeout,
            cancel,
            request.spec.wasi_capabilities().cloned(),
        )
        .await?),
    }
}

async fn run_persistent(
    source: String,
    memory_mib: u32,
    timeout: Duration,
    cancel: &CancellationToken,
    session_out: &mut Option<JsSession>,
) -> Result<GuestOutcome, V8WasmLeaseError> {
    let session = tokio::task::spawn_blocking(move || JsSession::start(memory_mib)).await??;
    let outcome = session.eval(source, timeout, cancel).await;
    if matches!(outcome, GuestOutcome::Completed { .. }) {
        *session_out = Some(session);
    } else {
        session.shutdown().await?;
    }
    Ok(outcome)
}

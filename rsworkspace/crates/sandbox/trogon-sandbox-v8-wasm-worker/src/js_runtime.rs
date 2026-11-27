use std::time::Duration;

use deno_core::v8;
use tokio_util::sync::CancellationToken;

use crate::constants::{GUEST_SCRIPT, MEMORY_LIMIT_MESSAGE, TERMINATE_RETRY};
use crate::js_guest::{bootstrap, drain_output, execute, install_heap_limit_guard, new_guest_runtime};
use crate::{CapturedOutput, GuestOutcome, JsRuntimeError};

/// Runs `source` in a fresh, deny-by-default V8 isolate: no filesystem, network, process, or
/// Deno ops reach the guest beyond a console print binding. `memory_mib` becomes a real V8
/// heap ceiling; `timeout` is wall-clock only, so a guest that spins the CPU without
/// yielding is bounded by wall time, not billed CPU time.
pub async fn run_js(
    source: String,
    memory_mib: u32,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<GuestOutcome, JsRuntimeError> {
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel::<v8::IsolateHandle>();
    let mut join = tokio::task::spawn_blocking(move || execute_in_isolate(source, memory_mib, handle_tx));
    let isolate_handle = handle_rx.await.ok();

    let stopped: fn(CapturedOutput) -> GuestOutcome = tokio::select! {
        result = &mut join => return result.map_err(|e| JsRuntimeError::TaskPanicked(e.to_string())),
        () = tokio::time::sleep(timeout) => GuestOutcome::timed_out,
        () = cancel.cancelled() => GuestOutcome::cancelled,
    };
    force_isolate_termination(isolate_handle.as_ref(), &mut join).await;
    Ok(stopped(CapturedOutput::default()))
}

/// Repeatedly calls `terminate_execution()` on `handle` until `join` completes.
///
/// A single request can be consumed between scripts or before the guest script starts
/// running, which would leave the guest running forever; repeating it on a short interval
/// guarantees one lands while the guest script is executing.
async fn force_isolate_termination<T>(handle: Option<&v8::IsolateHandle>, join: &mut tokio::task::JoinHandle<T>) {
    let mut finished = false;
    while !finished {
        handle.inspect(|handle| {
            handle.terminate_execution();
        });
        finished = tokio::time::timeout(TERMINATE_RETRY, &mut *join).await.is_ok();
    }
}

fn execute_in_isolate(
    source: String,
    memory_mib: u32,
    handle_tx: tokio::sync::oneshot::Sender<v8::IsolateHandle>,
) -> GuestOutcome {
    let mut runtime = new_guest_runtime(memory_mib);
    let handle = runtime.v8_isolate().thread_safe_handle();
    drop(handle_tx.send(handle.clone()));
    let out_of_memory = install_heap_limit_guard(&mut runtime, handle);

    let result = bootstrap(&mut runtime).and_then(|()| execute(&mut runtime, GUEST_SCRIPT, source.into()));
    let output = drain_output(&mut runtime);
    let reached_limit = out_of_memory.load(std::sync::atomic::Ordering::SeqCst);
    GuestOutcome::settle(result, output, reached_limit.then_some(MEMORY_LIMIT_MESSAGE))
}

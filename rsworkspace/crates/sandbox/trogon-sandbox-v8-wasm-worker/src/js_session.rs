use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use deno_core::v8;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::constants::{
    RETIRED_MESSAGE, SESSION_MEMORY_MESSAGE, SESSION_REQUEST_CAPACITY, SESSION_SCRIPT, START_TIMEOUT, TERMINATE_RETRY,
};
use crate::js_guest::{bootstrap, drain_output, execute, install_heap_limit_guard, new_guest_runtime};
use crate::{CapturedOutput, GuestOutcome, JsRuntimeError};

enum SessionRequest {
    Eval {
        source: String,
        reply: oneshot::Sender<GuestOutcome>,
    },
    Stop,
}

/// A JS runtime that outlives a single script: globals defined by one `eval` are visible to
/// the next. The runtime is not `Send`, so it lives on its own thread for the lifetime of
/// the session and dropping the session ends that thread and frees the isolate.
///
/// Memory is bounded by the V8 heap ceiling given at start. When a script reaches it the
/// script is terminated and the runtime is retired, because a heap that hit its ceiling
/// cannot be trusted to hold the state the caller expects.
pub struct JsSession {
    tx: mpsc::SyncSender<SessionRequest>,
    handle: v8::IsolateHandle,
    retired: Arc<AtomicBool>,
    shutdown_requested: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl JsSession {
    /// Starts the runtime thread and returns once the isolate exists.
    pub fn start(memory_mib: u32) -> Result<Self, JsRuntimeError> {
        let (tx, rx) = mpsc::sync_channel::<SessionRequest>(SESSION_REQUEST_CAPACITY);
        let (handle_tx, handle_rx) = mpsc::sync_channel::<v8::IsolateHandle>(1);
        let retired = Arc::new(AtomicBool::new(false));
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let thread_flags = (Arc::clone(&retired), Arc::clone(&shutdown_requested));
        // V8 posts delayed tasks that deno_core schedules on tokio, so the runtime thread
        // must run inside the caller's tokio context.
        let tokio_handle = tokio::runtime::Handle::try_current().map_err(|_| JsRuntimeError::NoTokioRuntime)?;

        let thread = std::thread::Builder::new()
            .name("v8-session".to_string())
            .spawn(move || {
                let _guard = tokio_handle.enter();
                session_thread(memory_mib, &rx, &handle_tx, &thread_flags.0, &thread_flags.1);
            })
            .map_err(JsRuntimeError::ThreadSpawn)?;
        let handle = handle_rx
            .recv_timeout(START_TIMEOUT)
            .map_err(|_| JsRuntimeError::StartTimeout)?;
        Ok(Self {
            tx,
            handle,
            retired,
            shutdown_requested,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Whether the runtime was retired (for example after exhausting its heap) and can no
    /// longer run scripts.
    pub fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    /// Whether the runtime thread has been joined.
    pub fn is_closed(&self) -> bool {
        self.thread.lock().unwrap_or_else(PoisonError::into_inner).is_none()
    }

    fn thread_finished(&self) -> bool {
        let thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
        thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Stops the runtime, interrupting a running script, and joins its thread.
    pub async fn shutdown(&self) -> Result<(), JsRuntimeError> {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        self.retired.store(true, Ordering::SeqCst);
        drop(self.tx.send(SessionRequest::Stop));
        loop {
            self.handle.terminate_execution();
            tokio::time::sleep(TERMINATE_RETRY).await;
            if self.thread_finished() {
                break;
            }
        }
        let thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner).take();
        thread.map_or(Ok(()), |join| join.join().map_err(|_| JsRuntimeError::ShutdownPanicked))
    }

    /// Runs `source` in the persistent runtime, bounded by `timeout` and `cancel`. A script
    /// that times out or is cancelled is terminated, and the runtime stays usable.
    pub async fn eval(&self, source: String, timeout: Duration, cancel: &CancellationToken) -> GuestOutcome {
        if self.shutdown_requested.load(Ordering::SeqCst) {
            return GuestOutcome::cancelled(CapturedOutput::default());
        }
        let (reply, mut reply_rx) = oneshot::channel();
        let queued = !self.is_retired() && self.tx.send(SessionRequest::Eval { source, reply }).is_ok();
        if !queued {
            return retired_outcome();
        }

        let stopped: fn(CapturedOutput) -> GuestOutcome = tokio::select! {
            result = &mut reply_rx => return result.unwrap_or_else(|_| retired_outcome()),
            () = tokio::time::sleep(timeout) => GuestOutcome::timed_out,
            () = cancel.cancelled() => GuestOutcome::cancelled,
        };

        // Terminate until the thread answers; one request can be consumed between scripts.
        let mut answer = None;
        while answer.is_none() {
            self.handle.terminate_execution();
            answer = tokio::time::timeout(TERMINATE_RETRY, &mut reply_rx).await.ok();
        }
        let outcome = answer.and_then(Result::ok).unwrap_or_else(retired_outcome);
        stopped(outcome.into_output())
    }
}

impl Drop for JsSession {
    fn drop(&mut self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        self.retired.store(true, Ordering::SeqCst);
        drop(self.tx.send(SessionRequest::Stop));
        let thread = self.thread.get_mut().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(thread) = thread {
            let mut finished = false;
            while !finished {
                self.handle.terminate_execution();
                std::thread::sleep(Duration::from_millis(1));
                finished = thread.is_finished();
            }
            drop(thread.join());
        }
    }
}

fn retired_outcome() -> GuestOutcome {
    GuestOutcome::failed(CapturedOutput::default(), RETIRED_MESSAGE)
}

fn session_thread(
    memory_mib: u32,
    rx: &mpsc::Receiver<SessionRequest>,
    handle_tx: &mpsc::SyncSender<v8::IsolateHandle>,
    retired: &AtomicBool,
    shutdown_requested: &AtomicBool,
) {
    let mut runtime = new_guest_runtime(memory_mib);
    let handle = runtime.v8_isolate().thread_safe_handle();
    drop(handle_tx.send(handle.clone()));
    let out_of_memory = install_heap_limit_guard(&mut runtime, handle);

    let booted = bootstrap(&mut runtime).map_err(|error| tracing::warn!(%error, "persistent runtime bootstrap failed"));
    retired.fetch_or(booted.is_err(), Ordering::SeqCst);

    while booted.is_ok()
        && let Ok(SessionRequest::Eval { source, reply }) = rx.recv()
    {
        // Clears a termination left over from the previous caller's stop loop.
        runtime.v8_isolate().cancel_terminate_execution();
        let result = execute(&mut runtime, SESSION_SCRIPT, source.into());
        let output = drain_output(&mut runtime);
        runtime.v8_isolate().cancel_terminate_execution();

        let reached_limit = out_of_memory.load(Ordering::SeqCst);
        retired.fetch_or(reached_limit, Ordering::SeqCst);
        let outcome = if shutdown_requested.load(Ordering::SeqCst) {
            GuestOutcome::cancelled(output)
        } else {
            GuestOutcome::settle(result, output, reached_limit.then_some(SESSION_MEMORY_MESSAGE))
        };
        drop(reply.send(outcome));
        if retired.load(Ordering::SeqCst) {
            break;
        }
    }
}

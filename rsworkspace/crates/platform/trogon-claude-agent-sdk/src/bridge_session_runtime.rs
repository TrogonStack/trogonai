//! The parts of an attached session that outlive one call: what the host and the background tasks both read.
//!
//! The upstream handle returns at once and uploads in the background, and its timers end the session on their own
//! (`onClose`). [`Lifecycle`] is the closed, ended and connected state with the host's close callback, shared through
//! an `Arc` so a task can end the session the way a call does. [`Uploads`] holds the frames the handle wrote and wakes
//! the upload task, which drains them into the worker and sends every queue until the service has taken it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use crate::adapters::HttpExecute;
use crate::bridge_session::{CloseCallback, client_event};
use crate::bridge_worker::{WorkerClient, WorkerPacing};
use crate::bridge_worker_events::ClientEvent;
use crate::bridge_worker_types::MismatchReason;

/// `x` (closed by the host), the transport having closed itself, and whether the worker is registered.
pub(crate) struct Lifecycle {
    closed: Arc<AtomicBool>,
    ended: AtomicBool,
    connected: AtomicBool,
    on_close: Mutex<Option<CloseCallback>>,
}

impl Lifecycle {
    pub(crate) fn new(on_close: Option<CloseCallback>) -> Self {
        Self {
            closed: Arc::new(AtomicBool::new(false)),
            ended: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            on_close: Mutex::new(on_close),
        }
    }

    /// The flag the heartbeat timer raises and reads.
    pub(crate) fn closed_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.closed)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub(crate) fn is_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst) && !self.is_ended() && !self.is_closed()
    }

    pub(crate) fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::SeqCst);
    }

    /// Raises the closed flag without anything else; false when it was already up.
    pub(crate) fn mark_closed(&self) -> bool {
        !self.closed.swap(true, Ordering::SeqCst)
    }

    /// A rebuilt transport starts again: neither ended nor connected.
    pub(crate) fn revive(&self) {
        self.ended.store(false, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
    }

    /// The transport closing itself: `wind_down` stops the worker's queues, then the host hears about it once,
    /// unless it closed the handle first or the transport already ended.
    pub(crate) fn end(&self, code: u16, wind_down: impl FnOnce()) {
        if self.is_closed() || self.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        self.connected.store(false, Ordering::SeqCst);
        wind_down();
        let mut on_close = self.on_close.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(callback) = on_close.as_mut() {
            callback(code);
        }
    }
}

/// Stops what the worker still holds; the same three calls whoever ends the session.
pub(crate) fn wind_down<H: HttpExecute, P: WorkerPacing>(worker: &WorkerClient<H, P>) {
    worker.close_queue();
    let _kept = worker.close_events(false);
    worker.close_deliveries();
}

/// The frames the handle wrote and the counters the upload task and its waiters agree on.
#[derive(Default)]
pub(crate) struct Uploads {
    frames: Mutex<Vec<Value>>,
    wake: Notify,
    requested: AtomicU64,
    drained: AtomicU64,
    idle: Notify,
}

impl Uploads {
    /// Queues a frame for the upload task.
    pub(crate) fn push(&self, frame: Value) {
        self.frames.lock().unwrap_or_else(PoisonError::into_inner).push(frame);
        self.wake();
    }

    pub(crate) fn take(&self) -> Vec<Value> {
        std::mem::take(&mut *self.frames.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Tells the upload task that something is queued; a wake before the task runs is kept.
    pub(crate) fn wake(&self) {
        self.requested.fetch_add(1, Ordering::SeqCst);
        self.wake.notify_one();
    }

    fn finish(&self, seen: u64) {
        self.drained.fetch_max(seen, Ordering::SeqCst);
        self.idle.notify_waiters();
    }

    /// Waits until the upload task has handled every wake so far. Only meaningful while a task runs.
    pub(crate) async fn settled(&self) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.drained.load(Ordering::SeqCst) >= self.requested.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

/// Starts the upload task: each wake moves the frames into the worker and sends the state and metadata reports, the
/// events and the acknowledgements, retrying with the worker's backoff. A terminal answer ends the session; the task
/// returns once the host closes it.
pub(crate) fn spawn_uploads<H, P>(
    uploads: Arc<Uploads>,
    lifecycle: Arc<Lifecycle>,
    worker: watch::Receiver<Arc<WorkerClient<H, P>>>,
) -> JoinHandle<()>
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    tokio::spawn(upload_loop(uploads, lifecycle, worker))
}

async fn upload_loop<H, P>(
    uploads: Arc<Uploads>,
    lifecycle: Arc<Lifecycle>,
    mut worker: watch::Receiver<Arc<WorkerClient<H, P>>>,
) where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    loop {
        uploads.wake.notified().await;
        let seen = uploads.requested.load(Ordering::SeqCst);
        if lifecycle.is_closed() {
            uploads.finish(seen);
            return;
        }
        if !lifecycle.is_ended() {
            let current = Arc::clone(&worker.borrow_and_update());
            drain(&uploads, &lifecycle, current.as_ref()).await;
        }
        uploads.finish(seen);
    }
}

/// `drain_outbox` and `pump` of the session, run by the task.
async fn drain<H: HttpExecute, P: WorkerPacing>(uploads: &Uploads, lifecycle: &Lifecycle, worker: &WorkerClient<H, P>) {
    let frames = uploads.take();
    if !frames.is_empty() {
        let events: Vec<ClientEvent> = frames.into_iter().filter_map(client_event).collect();
        let submitted = worker.submit_events(events).await;
        settle(lifecycle, worker, submitted);
    }
    let reports = worker.pump(None).await;
    settle(lifecycle, worker, reports);
    let events = worker.pump_events(None).await;
    settle(lifecycle, worker, events);
    let deliveries = worker.pump_deliveries(None).await;
    settle(lifecycle, worker, deliveries);
}

fn settle<H: HttpExecute, P: WorkerPacing, T>(
    lifecycle: &Lifecycle,
    worker: &WorkerClient<H, P>,
    result: Result<T, MismatchReason>,
) {
    if let Err(reason) = result {
        lifecycle.end(reason.close_code(false), || wind_down(worker));
    }
}

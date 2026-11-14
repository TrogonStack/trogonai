//! The heartbeat timer of the upstream worker client (`startHeartbeat`, bridge.mjs offset 1059000).
//!
//! With the options the attach entrypoint passes, the beat is a plain repeating timer: the delay is the current
//! interval, `Math.random` is still drawn once per schedule, a beat does not start while the previous one is
//! unanswered, an answer that advises a shorter interval restarts the timer at once (a longer one waits for the next
//! schedule), and the timer ends when the session closes or the service ends the worker. The probe beat, the jitter
//! fraction and the keep-alive floor of `startHeartbeat` need options the entrypoint never passes
//! (`advertiseHeartbeatProbeSupport`, `keepAlivesGoAlone`), so only the stream's `heartbeat_probe` frame can reach a
//! beat outside this schedule, and that belongs to the stream driver.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::adapters::HttpExecute;
use crate::bridge_worker::{WorkerClient, WorkerPacing};
use crate::bridge_worker_types::MismatchReason;

/// The longest delay a timer can have; a longer one is run after the shortest (`2147483647`).
const LARGEST_TIMER_MS: f64 = 2_147_483_647.0;

/// How long the timer waits before the next beat: the interval plus a spread of
/// `max(interval * jitter_fraction, jitter_floor_ms) * (2 * random - 1)`, at most [`LARGEST_TIMER_MS`]. A delay below
/// one millisecond runs after one, as a Node timer does.
pub fn heartbeat_delay(interval_ms: f64, jitter_fraction: f64, jitter_floor_ms: f64, random: f64) -> Duration {
    let spread = (interval_ms * jitter_fraction).max(jitter_floor_ms) * (2.0 * random - 1.0);
    let wait = (interval_ms + spread).min(LARGEST_TIMER_MS);
    let wait = if wait >= 1.0 { wait } else { 1.0 };
    Duration::from_secs_f64(wait / 1000.0)
}

struct Shared<H, P> {
    client: Arc<WorkerClient<H, P>>,
    closed: Arc<AtomicBool>,
    in_flight: AtomicBool,
    restart: Notify,
    on_terminal: Box<dyn Fn(MismatchReason) + Send + Sync>,
}

/// Beats on the worker's interval until the session closes or the service ends the worker.
pub struct HeartbeatDriver<H, P> {
    shared: Arc<Shared<H, P>>,
    ticker: JoinHandle<()>,
}

impl<H, P> HeartbeatDriver<H, P>
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    /// Starts the timer (`startHeartbeat`). `closed` is the session's closed flag, which also ends the timer when the
    /// session raises it. `on_terminal` runs once when the service answers a beat in a way that ends the worker
    /// (`onEpochMismatch`); the timer has stopped by then.
    pub fn start(
        client: Arc<WorkerClient<H, P>>,
        closed: Arc<AtomicBool>,
        on_terminal: impl Fn(MismatchReason) + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            client,
            closed,
            in_flight: AtomicBool::new(false),
            restart: Notify::new(),
            on_terminal: Box::new(on_terminal),
        });
        let ticker = tokio::spawn(tick_loop(Arc::clone(&shared)));
        Self { shared, ticker }
    }

    /// Stops the timer (`stopHeartbeat`). A beat already sent still gets its answer.
    pub fn stop(&self) {
        self.ticker.abort();
    }
}

impl<H, P> Drop for HeartbeatDriver<H, P> {
    fn drop(&mut self) {
        self.ticker.abort();
    }
}

/// The timer callback and the `e()` that schedules it: each tick starts a beat without waiting for it, then draws the
/// next delay. A restart request reschedules at once, the way `startHeartbeat` does when it is called while running.
async fn tick_loop<H, P>(shared: Arc<Shared<H, P>>)
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    while !shared.closed.load(Ordering::Acquire) {
        let pacing = shared.client.pacing();
        let delay = heartbeat_delay(shared.client.heartbeat_interval_ms(), 0.0, 0.0, pacing.jitter());
        tokio::select! {
            () = pacing.pause(delay) => {
                if shared.closed.load(Ordering::Acquire) {
                    return;
                }
                tokio::spawn(beat(Arc::clone(&shared)));
            }
            () = shared.restart.notified() => {}
        }
    }
}

/// `sendHeartbeat`: one beat unless the previous one is still unanswered.
async fn beat<H, P>(shared: Arc<Shared<H, P>>)
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    if shared.in_flight.swap(true, Ordering::AcqRel) {
        return;
    }
    let before = shared.client.heartbeat_interval_ms();
    let outcome = shared.client.heartbeat().await;
    shared.in_flight.store(false, Ordering::Release);
    match outcome {
        Err(reason) => {
            shared.closed.store(true, Ordering::Release);
            shared.restart.notify_one();
            (shared.on_terminal)(reason);
        }
        Ok(_) if shared.client.heartbeat_interval_ms() < before => shared.restart.notify_one(),
        Ok(_) => {}
    }
}

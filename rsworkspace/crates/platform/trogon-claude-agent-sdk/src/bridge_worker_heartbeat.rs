//! The heartbeat timer of the upstream worker client (`startHeartbeat`, bridge.mjs offset 1059000).
//!
//! With the options the attach entrypoint passes, the beat is a plain repeating timer: the delay is the current
//! interval, `Math.random` is still drawn once per schedule, a beat does not start while the previous one is
//! unanswered, an answer that advises a shorter interval restarts the timer at once (a longer one waits for the next
//! schedule), and the timer ends when the session closes or the service ends the worker. The probe beat, the jitter
//! fraction and the keep-alive floor of `startHeartbeat` need options the entrypoint never passes
//! (`advertiseHeartbeatProbeSupport`, `keepAlivesGoAlone`), so only the stream's `heartbeat_probe` frame can reach a
//! beat outside this schedule: [`HeartbeatDriver::probe`] is `heartbeatNow("probe")`, which sends at once, waits out
//! the spacing since the last beat, or waits for the beat in flight.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::adapters::HttpExecute;
use crate::bridge_worker::{WorkerClient, WorkerPacing};
use crate::bridge_worker_types::MismatchReason;

mod constants;

use constants::LARGEST_TIMER_MS;
pub use constants::PROBE_BEAT_SPACING;

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
    last_sent: Mutex<Option<Instant>>,
    pending_forced: AtomicBool,
    probe_timer: Mutex<Option<JoinHandle<()>>>,
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
            last_sent: Mutex::new(None),
            pending_forced: AtomicBool::new(false),
            probe_timer: Mutex::new(None),
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

    /// `heartbeatNow("probe")`, what the service's `heartbeat_probe` frame asks for. A closed session does nothing; a
    /// beat in flight is followed by one more when it ends; a beat sent less than [`PROBE_BEAT_SPACING`] ago is
    /// followed by one when the spacing has passed (one waiting timer however many probes come); otherwise the beat
    /// goes out now.
    pub fn probe(&self) {
        heartbeat_now(&self.shared);
    }
}

impl<H, P> Drop for HeartbeatDriver<H, P> {
    fn drop(&mut self) {
        self.ticker.abort();
        clear_probe_timer(&self.shared.probe_timer);
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
                send_heartbeat(&shared);
            }
            () = shared.restart.notified() => {}
        }
    }
}

fn clear_probe_timer(timer: &Mutex<Option<JoinHandle<()>>>) {
    if let Some(handle) = timer.lock().unwrap_or_else(PoisonError::into_inner).take() {
        handle.abort();
    }
}

fn heartbeat_now<H, P>(shared: &Arc<Shared<H, P>>)
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    if shared.closed.load(Ordering::Acquire) {
        return;
    }
    if shared.in_flight.load(Ordering::Acquire) {
        shared.pending_forced.store(true, Ordering::Release);
        return;
    }
    let since = shared
        .last_sent
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .map(|sent| sent.elapsed());
    if let Some(since) = since.filter(|since| *since < PROBE_BEAT_SPACING) {
        let mut timer = shared.probe_timer.lock().unwrap_or_else(PoisonError::into_inner);
        if timer.is_some() {
            return;
        }
        let again = Arc::clone(shared);
        let wait = PROBE_BEAT_SPACING - since;
        *timer = Some(tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            again.probe_timer.lock().unwrap_or_else(PoisonError::into_inner).take();
            heartbeat_now(&again);
        }));
        return;
    }
    clear_probe_timer(&shared.probe_timer);
    send_heartbeat(shared);
}

/// `sendHeartbeat`: one beat unless the previous one is still unanswered. The claim on the beat and its time are
/// taken here, before the request is spawned, as the upstream method takes them before its first await.
fn send_heartbeat<H, P>(shared: &Arc<Shared<H, P>>)
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    if shared.closed.load(Ordering::Acquire) || shared.in_flight.swap(true, Ordering::AcqRel) {
        return;
    }
    *shared.last_sent.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    tokio::spawn(beat(Arc::clone(shared)));
}

async fn beat<H, P>(shared: Arc<Shared<H, P>>)
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    let before = shared.client.heartbeat_interval_ms();
    let outcome = shared.client.heartbeat().await;
    match outcome {
        Err(reason) => {
            shared.closed.store(true, Ordering::Release);
            shared.restart.notify_one();
            (shared.on_terminal)(reason);
        }
        Ok(_) if shared.client.heartbeat_interval_ms() < before => shared.restart.notify_one(),
        Ok(_) => {}
    }
    shared.in_flight.store(false, Ordering::Release);
    if shared.pending_forced.swap(false, Ordering::AcqRel) && !shared.closed.load(Ordering::Acquire) {
        heartbeat_now(&shared);
    }
}

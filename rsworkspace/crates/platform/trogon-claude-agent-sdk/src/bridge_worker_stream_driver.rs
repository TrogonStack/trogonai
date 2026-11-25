//! The timers around the read stream (`openStream`, `readStream`, `handleConnectionError` and `onLivenessTimeout` of
//! the `SSETransport` class `AC`, bridge.mjs offset 1162854): how long a request may take to answer, how long the body
//! of a refusal is read, when a silent stream is declared dead and how long the next request waits.
//!
//! [`run_stream`] owns the loop and the clocks and asks a [`StreamConnector`] for each request, so the HTTP layer is
//! swapped for a scripted one in tests. What the frames ask for goes to a sink as [`StreamEffect`] values; the
//! decisions about statuses, sequence numbers and reconnect counting stay in [`StreamSession`]. The recorded
//! timelines (`stream` cases of `timelines.json`) drive all of it under a paused Tokio clock.
//!
//! Not reachable from `attachBridgeSession` and so not ported: the sleep notice and its frame wait timer
//! (`onProcessSlept` is not passed), the long poll mode and the held-for-renewal gate.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::{Instant, sleep, timeout, timeout_at};

use crate::bridge_worker_stream::{Reopen, StreamEffect, StreamOpen, StreamRequest, StreamSession, liveness_timeout};

mod constants;

pub use constants::{CONNECT_TIMEOUT, REFUSAL_BODY_TIMEOUT, REOPEN_AT_ONCE};

/// A request or a body that failed. The cause is only for the log: every failure is retried the same way.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the stream failed: {0}")]
pub struct StreamFailureError(pub String);

/// The answer to a request for the stream.
pub struct StreamResponse<B> {
    pub status: u16,
    pub body: B,
}

/// The body of an answer, read a chunk at a time.
pub trait StreamBody: Send {
    /// The next chunk: `None` when the body ended, an error when it broke.
    fn next_chunk(&mut self) -> impl Future<Output = Option<Result<Vec<u8>, StreamFailureError>>> + Send;
}

/// Sends the request for the stream.
pub trait StreamConnector: Send + Sync {
    type Body: StreamBody;

    /// Sends one request and resolves when the status and headers arrive.
    fn open(
        &self,
        request: &StreamRequest,
    ) -> impl Future<Output = Result<StreamResponse<Self::Body>, StreamFailureError>> + Send;
}

/// The two clocks the liveness check compares: `Date.now` and `performance.now`, each as a time since a fixed
/// origin. Only the differences are used.
#[derive(Clone)]
pub struct StreamClocks {
    pub wall: Arc<dyn Fn() -> Duration + Send + Sync>,
    pub monotonic: Arc<dyn Fn() -> Duration + Send + Sync>,
}

impl Default for StreamClocks {
    fn default() -> Self {
        let base = Instant::now();
        Self {
            wall: Arc::new(|| SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()),
            monotonic: Arc::new(move || base.elapsed()),
        }
    }
}

/// The session behind its lock. The lock is never held across an await: the host reads the sequence number and
/// replaces the session on a reconnect while the loop waits.
fn held(session: &Mutex<StreamSession>) -> MutexGuard<'_, StreamSession> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A delay as a Node timer takes it: whole milliseconds, at least one.
fn whole_milliseconds(delay: Duration) -> Duration {
    Duration::from_millis(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX).max(1))
}

/// Reads the stream until the service refuses it for good, and returns the status to close the session with. The
/// future never ends otherwise: the caller drops it when the session closes.
pub async fn run_stream<C: StreamConnector>(
    connector: &C,
    session: &Mutex<StreamSession>,
    clocks: &StreamClocks,
    random: &mut (dyn FnMut() -> f64 + Send),
    sink: &mut (dyn FnMut(StreamEffect) + Send),
) -> u16 {
    loop {
        let request = held(session).request();
        let opened = timeout(CONNECT_TIMEOUT, connector.open(&request)).await;
        let wait = match opened {
            Err(_) | Ok(Err(_)) => lost(session, random),
            Ok(Ok(response)) => {
                let open = held(session).opened(response.status);
                match open {
                    StreamOpen::Read => read(response.body, session, clocks, random, sink).await,
                    StreamOpen::Retry => {
                        drain(response.body).await;
                        lost(session, random)
                    }
                    StreamOpen::Close(code) => {
                        drain(response.body).await;
                        return code;
                    }
                }
            }
        };
        sleep(whole_milliseconds(wait)).await;
    }
}

fn lost(session: &Mutex<StreamSession>, random: &mut (dyn FnMut() -> f64 + Send)) -> Duration {
    let draw = random();
    held(session).lost(draw)
}

/// Reads the body of a refusal for at most [`REFUSAL_BODY_TIMEOUT`].
async fn drain<B: StreamBody>(mut body: B) {
    let _ = timeout(REFUSAL_BODY_TIMEOUT, async {
        while let Some(Ok(_)) = body.next_chunk().await {}
    })
    .await;
}

/// What the liveness timer is measured from: the last connect or frame, on both clocks.
struct Evidence {
    at: Instant,
    wall: Duration,
    monotonic: Duration,
}

impl Evidence {
    fn now(clocks: &StreamClocks) -> Self {
        Self {
            at: Instant::now(),
            wall: (clocks.wall)(),
            monotonic: (clocks.monotonic)(),
        }
    }
}

/// Reads one body and returns how long to wait before the next request.
async fn read<B: StreamBody>(
    mut body: B,
    session: &Mutex<StreamSession>,
    clocks: &StreamClocks,
    random: &mut (dyn FnMut() -> f64 + Send),
    sink: &mut (dyn FnMut(StreamEffect) + Send),
) -> Duration {
    let mut evidence = Evidence::now(clocks);
    loop {
        let chunk = match timeout_at(evidence.at + liveness_timeout(), body.next_chunk()).await {
            Ok(chunk) => chunk,
            Err(_) => {
                let wall = (clocks.wall)().saturating_sub(evidence.wall);
                let monotonic = (clocks.monotonic)().saturating_sub(evidence.monotonic);
                let reopen = held(session).liveness_expired_with(wall, monotonic, random);
                return match reopen {
                    Reopen::AtOnce { .. } => REOPEN_AT_ONCE,
                    Reopen::After(delay) => delay,
                };
            }
        };
        match chunk {
            Some(Ok(bytes)) => {
                let (frames, effects) = held(session).push_counted(&bytes);
                if frames > 0 {
                    evidence = Evidence::now(clocks);
                }
                for effect in effects {
                    sink(effect);
                }
            }
            Some(Err(_)) | None => return lost(session, random),
        }
    }
}

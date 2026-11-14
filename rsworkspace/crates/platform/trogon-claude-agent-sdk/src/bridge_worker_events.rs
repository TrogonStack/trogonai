//! The client event queue (`Su`, bridge.mjs offset 988814) behind `POST /worker/events`: events wait their turn, go
//! out in batches bounded by count and bytes, and are retried with a backoff until the service takes them.
//!
//! The queue does no I/O. [`EventQueue::poll`] says what to do next (send a batch, wait, or nothing) and
//! [`EventQueue::done`] or [`EventQueue::failed`] take the answer, which is the order the upstream `drain` loop runs
//! in. The pieces of `Su` that only a flush policy, the urgent lane or the worker socket reach (`setHold`, `urgent`,
//! `waitForMoreMs`, `longWaitForRoom`, `maxConsecutiveFailures`, `resendSizes`) are left out: the bridge entrypoint
//! never sets them for the client event uploader (offsets 1034000 to 1035500), and they go with the PRs that
//! port those features.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use serde_json::{Map, Value};

mod constants;

use constants::{BASE_DELAY_MS, JITTER_MS, MAX_DELAY_MS, MIB, SECONDS_PER_MIB, TIMEOUT_GROWTH};
pub use constants::{
    MAX_BATCH_BYTES, MAX_BATCH_EVENTS, MAX_QUEUE_EVENTS, POST_TIMEOUT_CEILING_MS, POST_TIMEOUT_FLOOR_MS,
    SHRINK_FROM_BYTES,
};

/// The lane a request path belongs to (`JA`, offset 1013965). The path of the batched upload (`oC`) is
/// `/worker/event-batch`.
pub fn lane_for_path(path: &str) -> Option<&'static str> {
    match path {
        "/worker" => Some("worker_state"),
        "/worker/event-batch" | "/worker/events" => Some("events"),
        "/worker/events/delivery" => Some("delivery"),
        "/worker/heartbeat" => Some("heartbeat"),
        "/worker/internal-events" => Some("internal_events"),
        _ => None,
    }
}

/// How the request timeout of one upload is set (`a5`, offset 1011496): ten seconds plus five per mebibyte, grown by
/// one and a half for every earlier failure when a large batch cannot be cut, kept within the floor and ceiling.
pub fn upload_timeout(bytes: usize, cuttable: bool, retry_count: u32) -> Duration {
    let base = POST_TIMEOUT_FLOOR_MS as f64 + (bytes as f64 / MIB as f64 * SECONDS_PER_MIB).round() * 1000.0;
    let grown = if bytes >= SHRINK_FROM_BYTES && !cuttable {
        base * TIMEOUT_GROWTH.powi(i32::try_from(retry_count).unwrap_or(i32::MAX))
    } else {
        base
    };
    let ms = grown
        .round()
        .min(POST_TIMEOUT_CEILING_MS as f64)
        .max(POST_TIMEOUT_FLOOR_MS as f64);
    Duration::from_millis(ms as u64)
}

/// One event as it is uploaded: the payload, whether it replays history (`historical: true` on the row), and
/// whether it is a live-view row the store does not keep.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientEvent {
    pub payload: Map<String, Value>,
    pub historical: bool,
    pub ephemeral: bool,
}

impl ClientEvent {
    pub fn to_json(&self) -> Value {
        let mut event = Map::new();
        event.insert("payload".to_owned(), Value::Object(self.payload.clone()));
        if self.historical {
            event.insert("historical".to_owned(), Value::Bool(true));
        }
        if self.ephemeral {
            event.insert("ephemeral".to_owned(), Value::Bool(true));
        }
        Value::Object(event)
    }

    /// The bytes the event takes in a request (`aa`).
    pub fn json_bytes(&self) -> usize {
        crate::settings_json::js_stringify(&self.to_json()).len()
    }
}

/// Whether the service may have seen a batch's events before (`x-ccr-upload-attempt`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Attempt {
    First,
    Resend,
    ResendWithNew,
}

impl Attempt {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Resend => "resend",
            Self::ResendWithNew => "resend_with_new",
        }
    }
}

/// One batch to send.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub events: Vec<ClientEvent>,
    pub bytes: usize,
    pub attempt: Attempt,
    /// Failures in a row before this send (`retryCount`).
    pub retry_count: u32,
    /// Whether a batch of several events could be cut (`cuttable`).
    pub cuttable: bool,
}

/// What the queue wants done next.
#[derive(Clone, Debug, PartialEq)]
pub enum EventNext {
    Send(Batch),
    Pause(Duration),
    Idle,
}

#[derive(Debug)]
struct Queued {
    id: u64,
    event: ClientEvent,
}

#[derive(Debug)]
struct InFlight {
    items: Vec<Queued>,
    /// How many events the batch had when it was taken, and their bytes.
    taken: usize,
    bytes: usize,
}

/// The queue of client events.
#[derive(Debug)]
pub struct EventQueue {
    pending: VecDeque<Queued>,
    in_flight: Option<InFlight>,
    sent_before: HashSet<u64>,
    next_id: u64,
    failures: u32,
    /// The byte limit a request with no answer has cut the next batches to (`t` in `drain`).
    shrunk_to: Option<usize>,
    wait: Option<Duration>,
    max_queue: usize,
    closed: bool,
    undelivered: Vec<ClientEvent>,
    retaining: bool,
    handed_over_after_close: usize,
    pending_at_close: usize,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new(MAX_QUEUE_EVENTS)
    }
}

impl EventQueue {
    pub fn new(max_queue: usize) -> Self {
        Self {
            pending: VecDeque::new(),
            in_flight: None,
            sent_before: HashSet::new(),
            next_id: 0,
            failures: 0,
            shrunk_to: None,
            wait: None,
            max_queue,
            closed: false,
            undelivered: Vec::new(),
            retaining: false,
            handed_over_after_close: 0,
            pending_at_close: 0,
        }
    }

    /// The most events the queue holds.
    pub fn capacity(&self) -> usize {
        self.max_queue
    }

    /// Whether `count` more events fit (`hasRoomFor`).
    pub fn has_room_for(&self, count: usize) -> bool {
        !self.closed && self.pending.len() + count <= self.max_queue
    }

    /// Events not yet sent, counting the batch in flight.
    pub fn unsent(&self) -> usize {
        self.in_flight.as_ref().map_or(0, |batch| batch.items.len()) + self.pending.len()
    }

    /// Events waiting (`pendingCount`); once closed, how many were waiting at that moment.
    pub fn pending_count(&self) -> usize {
        if self.closed {
            self.pending_at_close
        } else {
            self.pending.len()
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Events handed over after the queue closed (`handedOverAfterClose`).
    pub fn handed_over_after_close(&self) -> usize {
        self.handed_over_after_close
    }

    /// Queues events. After the close they are kept for the caller only when the close asked for that.
    pub fn enqueue(&mut self, events: Vec<ClientEvent>) {
        if events.is_empty() {
            return;
        }
        if self.closed {
            self.retain_undelivered(events);
            return;
        }
        for event in events {
            let id = self.next_id;
            self.next_id += 1;
            self.pending.push_back(Queued { id, event });
        }
    }

    fn retain_undelivered(&mut self, events: Vec<ClientEvent>) {
        self.handed_over_after_close += events.len();
        if !self.retaining {
            return;
        }
        let room = self.max_queue.saturating_sub(self.undelivered.len());
        self.undelivered.extend(events.into_iter().take(room));
    }

    /// Closes the queue (`close`). With `retain` the events still waiting are kept, in order, for
    /// [`EventQueue::take_undelivered`].
    pub fn close(&mut self, retain: bool) -> usize {
        if self.closed {
            return 0;
        }
        self.closed = true;
        self.pending_at_close = self.pending.len();
        self.retaining = retain;
        self.sent_before.clear();
        self.wait = None;
        let mut all: Vec<ClientEvent> = self
            .in_flight
            .take()
            .map(|batch| batch.items.into_iter().map(|queued| queued.event).collect())
            .unwrap_or_default();
        all.extend(self.pending.drain(..).map(|queued| queued.event));
        let count = all.len();
        self.undelivered = if retain { all } else { Vec::new() };
        count
    }

    pub fn take_undelivered(&mut self) -> Vec<ClientEvent> {
        std::mem::take(&mut self.undelivered)
    }

    /// Whether [`EventQueue::poll`] would have nothing to do.
    pub fn is_idle(&self) -> bool {
        self.closed || (self.wait.is_none() && self.pending.is_empty())
    }

    /// What to do next.
    pub fn poll(&mut self) -> EventNext {
        if self.closed {
            return EventNext::Idle;
        }
        if let Some(delay) = self.wait.take() {
            return EventNext::Pause(delay);
        }
        if self.pending.is_empty() {
            return EventNext::Idle;
        }
        let limit = self.byte_limit();
        let mut bytes = 0;
        let mut count = 0;
        while count < self.pending.len() && count < MAX_BATCH_EVENTS {
            let size = self.pending[count].event.json_bytes();
            if count > 0 && bytes + size > limit {
                break;
            }
            bytes += size;
            count += 1;
        }
        let items: Vec<Queued> = self.pending.drain(..count).collect();
        let (attempt, _) = self.take_attempt_of(&items);
        let cuttable = items.len() > 1;
        let events = items.iter().map(|queued| queued.event.clone()).collect();
        self.in_flight = Some(InFlight {
            taken: items.len(),
            items,
            bytes,
        });
        EventNext::Send(Batch {
            events,
            bytes,
            attempt,
            retry_count: self.failures,
            cuttable,
        })
    }

    fn byte_limit(&self) -> usize {
        self.shrunk_to
            .map_or(MAX_BATCH_BYTES, |shrunk| shrunk.min(MAX_BATCH_BYTES))
    }

    fn take_attempt_of(&mut self, items: &[Queued]) -> (Attempt, usize) {
        if self.sent_before.is_empty() {
            return (Attempt::First, 0);
        }
        let again = items
            .iter()
            .filter(|queued| self.sent_before.remove(&queued.id))
            .count();
        let attempt = if again == 0 {
            Attempt::First
        } else if again < items.len() {
            Attempt::ResendWithNew
        } else {
            Attempt::Resend
        };
        (attempt, again)
    }

    /// The batch in flight was taken (or dropped for good): the next batch goes out at once.
    pub fn done(&mut self) {
        self.failures = 0;
        self.in_flight = None;
    }

    /// Drops the events of the batch in flight that `durable` rejects and keeps the rest for the retry; returns how
    /// many were dropped. Used when the service refuses a batch for good and the live-view rows are let go.
    pub fn keep_only(&mut self, durable: impl Fn(&ClientEvent) -> bool) -> usize {
        let Some(batch) = self.in_flight.as_mut() else {
            return 0;
        };
        let before = batch.items.len();
        batch.items.retain(|queued| durable(&queued.event));
        before - batch.items.len()
    }

    /// The events still in flight, after [`EventQueue::keep_only`].
    pub fn in_flight_events(&self) -> Vec<ClientEvent> {
        self.in_flight
            .as_ref()
            .map(|batch| batch.items.iter().map(|queued| queued.event.clone()).collect())
            .unwrap_or_default()
    }

    /// The batch in flight failed and goes back to the front of the queue, `jitter` being a fresh value in `[0, 1)`.
    /// `retry_after_ms` is what a `429` asked for; `unanswered` is true when no answer came, which halves the byte
    /// limit from [`SHRINK_FROM_BYTES`] on.
    pub fn failed(&mut self, jitter: f64, retry_after_ms: Option<f64>, unanswered: bool) {
        // A closed queue holds no batch in flight: `close` takes it.
        let Some(batch) = self.in_flight.take() else {
            return;
        };
        self.failures += 1;
        for queued in &batch.items {
            self.sent_before.insert(queued.id);
        }
        if unanswered {
            let bytes = if batch.items.len() == batch.taken {
                batch.bytes
            } else {
                batch.items.iter().map(|queued| queued.event.json_bytes()).sum()
            };
            if bytes >= SHRINK_FROM_BYTES {
                let half = SHRINK_FROM_BYTES.max(bytes / 2);
                self.shrunk_to = Some(self.shrunk_to.map_or(half, |earlier| earlier.min(half)));
            }
        }
        for queued in batch.items.into_iter().rev() {
            self.pending.push_front(queued);
        }
        self.wait = Some(retry_delay(self.failures, retry_after_ms, jitter));
    }
}

/// How long to wait after the `failures`-th failure in a row (`retryDelay`, offset 1124500): the wait a `429` asked
/// for, kept between the base and the ceiling, or the doubling delay, plus the jitter.
pub fn retry_delay(failures: u32, retry_after_ms: Option<f64>, jitter: f64) -> Duration {
    let extra = jitter * JITTER_MS;
    let ms = match retry_after_ms {
        Some(asked) if asked.is_finite() => asked.clamp(BASE_DELAY_MS, MAX_DELAY_MS) + extra,
        _ => {
            (BASE_DELAY_MS * 2_f64.powi(i32::try_from(failures.saturating_sub(1)).unwrap_or(i32::MAX)))
                .min(MAX_DELAY_MS)
                + extra
        }
    };
    Duration::from_micros((ms * 1000.0).round() as u64)
}

/// A `400`, `413` or `422` answer: the service refuses the batch for good (`Wn`).
pub fn refused_for_good(status: u16) -> bool {
    matches!(status, 400 | 413 | 422)
}

/// The body of one `POST /worker/events`.
pub fn upload_body(epoch: i64, events: &[ClientEvent]) -> Value {
    let mut body = Map::new();
    body.insert("worker_epoch".to_owned(), Value::from(epoch));
    body.insert(
        "events".to_owned(),
        Value::Array(events.iter().map(ClientEvent::to_json).collect()),
    );
    Value::Object(body)
}

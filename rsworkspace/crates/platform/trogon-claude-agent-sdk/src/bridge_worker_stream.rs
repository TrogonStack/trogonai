//! The read side of the worker client (`GET <session>/worker/events/stream`, the `SSETransport` class `AC` in
//! bridge.mjs, offset 1010000 to 1030000): how the stream is opened, how its bytes become frames, what each frame
//! does, and how a lost stream is reopened.
//!
//! The session does no I/O. A caller opens the request [`StreamSession::request`] describes, reports the status with
//! [`StreamSession::opened`], pushes the body with [`StreamSession::push`] and acts on the [`StreamEffect`] values it
//! returns, and reports a stream that ended or failed to open with [`StreamSession::lost`], which says how long to
//! wait before the next request. The recorded goldens (`stream` section) drive all of it through the real
//! `attachBridgeSession` against a local server.
//!
//! `attachBridgeSession` never passes the options of the transport that would change this: `nonOrigin403` (the
//! windowed refusal policy), `selfHealHeartbeats`, `causeTypedCloseCodes` (the 4093 and 4094 close codes),
//! `longPollFallback`, `streamOpener`, `leavesPool`, the attestation filter and the held-for-renewal gate. None is
//! ported. The lines [`StreamEffect::Deliver`] carries are routed by [`crate::InboundRouter`], which drops a user message
//! whose `uuid` was seen.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;

use crate::{bridge_js::js_parse_int, sse_frame::SseFrame};

mod constants;

pub use constants::{
    KEEPALIVE_SECONDS, LIVENESS_GRACE_MS, PERMANENT_STREAM_STATUSES, RECONNECT_BASE_MS, RECONNECT_CAP_MS,
    RECONNECT_JITTER_RATIO, SLEEP_LATENESS_MS,
};
use constants::{SEEN_LIMIT, SEEN_WINDOW, SERVER_ONLY_TYPES};

/// The wait before reconnect `attempt` (1 for the first), for a random draw in `[0, 1)` (`fl` with `symmetric`).
pub fn reconnect_delay(attempt: u32, random: f64) -> Duration {
    let exponent = i32::try_from(attempt.saturating_sub(1)).unwrap_or(i32::MAX);
    let raw = (RECONNECT_BASE_MS as f64 * 2f64.powi(exponent)).min(RECONNECT_CAP_MS as f64);
    let ms = (raw + raw * RECONNECT_JITTER_RATIO * (2.0 * random - 1.0)).max(0.0);
    Duration::from_secs_f64(ms / 1_000.0)
}

/// How long the stream may stay silent before it is reopened (`livenessTimeoutMs`): the keepalive interval plus the
/// grace.
pub fn liveness_timeout() -> Duration {
    Duration::from_millis(KEEPALIVE_SECONDS * 1_000 + LIVENESS_GRACE_MS)
}

/// The line terminator at `at`: its length, `Some(0)` for none, `None` when a lone `\r` ends the text and the next
/// byte could still make it `\r\n` (`J5`).
fn terminator(text: &[u8], at: usize) -> Option<usize> {
    match text.get(at) {
        Some(b'\n') => Some(1),
        Some(b'\r') => match text.get(at + 1) {
            None => None,
            Some(b'\n') => Some(2),
            Some(_) => Some(1),
        },
        _ => Some(0),
    }
}

/// The first blank line in `text`: where the frame ends and where the next begins (`R4e`).
fn blank_line(text: &[u8]) -> Option<(usize, usize)> {
    let mut at = 0;
    while at < text.len() {
        let first = terminator(text, at)?;
        if first == 0 {
            at += 1;
            continue;
        }
        let next = at + first;
        if next >= text.len() {
            return None;
        }
        let second = terminator(text, next)?;
        if second > 0 {
            return Some((at, next + second));
        }
        at = next;
    }
    None
}

/// One frame's text to its fields (`Q5`): `None` for blank text and for a frame with no data and no comment.
fn parse_frame(text: &str) -> Option<SseFrame> {
    if text.trim().is_empty() {
        return None;
    }
    let mut frame = SseFrame::default();
    let mut commented = false;
    let mut data = String::new();
    for line in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        if line.starts_with(':') {
            commented = true;
            continue;
        }
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let value = rest.strip_prefix(' ').unwrap_or(rest);
        match name {
            "event" => frame.event = Some(value.to_owned()),
            "id" => frame.id = Some(value.to_owned()),
            "data" => {
                data = if data.is_empty() {
                    value.to_owned()
                } else {
                    format!("{data}\n{value}")
                }
            }
            _ => {}
        }
    }
    if !data.is_empty() {
        frame.data = Some(data);
    }
    (frame.data.is_some() || commented).then_some(frame)
}

/// Turns the bytes of one response body into frames (`dC`). A frame ends at a blank line; a text that stops in the
/// middle of a frame, of a UTF-8 character or after a lone `\r` waits for the next push.
#[derive(Debug, Default)]
pub struct FrameParser {
    carry: Vec<u8>,
    pending: String,
}

impl FrameParser {
    /// Adds a chunk of the body.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<SseFrame> {
        self.carry.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.carry) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => self.carry.len(),
        };
        let rest = self.carry.split_off(valid);
        let decoded = String::from_utf8_lossy(&std::mem::replace(&mut self.carry, rest)).into_owned();
        self.push_str(&decoded)
    }

    /// Adds a chunk of text.
    pub fn push_str(&mut self, text: &str) -> Vec<SseFrame> {
        if text.is_empty() {
            return Vec::new();
        }
        self.pending.push_str(text);
        let mut frames = Vec::new();
        let mut from = 0;
        while let Some((end, after)) = blank_line(&self.pending.as_bytes()[from..]) {
            if let Some(frame) = parse_frame(&self.pending[from..from + end]) {
                frames.push(frame);
            }
            from += after;
        }
        self.pending.drain(..from);
        frames
    }

    /// The text of the frame not yet complete.
    pub fn buffered(&self) -> &str {
        &self.pending
    }
}

/// What a caller does for a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEffect {
    /// Hand this line (the payload as JSON and a newline) to the inbound reader (`onData`).
    Deliver(String),
    /// Acknowledge the event as `received` and then `processed` (`reportDelivery`).
    Acknowledge { event_id: String },
    /// Send a heartbeat now (`heartbeatNow("probe")`).
    HeartbeatProbe,
}

/// The answer to the request for the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamOpen {
    /// Read the body.
    Read,
    /// Wait [`StreamSession::lost`] and ask again.
    Retry,
    /// Close the session with this code.
    Close(u16),
}

/// What the request for the stream carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamRequest {
    /// The query of the URL after `/worker/events/stream`: `from_sequence_num=<n>` once an event arrived.
    pub query: Option<String>,
    /// The `Last-Event-ID` header, set with the query.
    pub last_event_id: Option<String>,
}

/// What a stream that went silent is reopened with ([`StreamSession::liveness_expired`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reopen {
    /// The check ran so late that the process was probably paused or asleep: the old connection is dropped and the
    /// next one is opened at once, on a new socket, with no backoff and without counting a failed attempt. `late` is
    /// how long past the liveness timeout the check ran.
    AtOnce { late: Duration },
    /// The stream was silent for the whole liveness timeout: wait this long, like any other lost stream.
    After(Duration),
}

/// The state of one session's read stream: the sequence numbers seen, the reconnect attempts and the parser of the
/// body being read.
#[derive(Debug, Default)]
pub struct StreamSession {
    connected: bool,
    last_sequence: i64,
    seen: BTreeSet<i64>,
    attempts: u32,
    parser: FrameParser,
}

impl StreamSession {
    /// A session that resumes after `initial` (`initialSequenceNum`): a number above zero seeds the sequence the
    /// first request resumes from, anything else starts from the beginning.
    #[must_use]
    pub fn seeded(initial: Option<i64>) -> Self {
        Self {
            last_sequence: initial.filter(|sequence| *sequence > 0).unwrap_or(0),
            ..Self::default()
        }
    }

    /// The highest sequence number of a durable event seen (`getSequenceNum`).
    pub fn last_sequence(&self) -> i64 {
        self.last_sequence
    }

    /// The request to open the stream with: it resumes after the last sequence number seen.
    pub fn request(&self) -> StreamRequest {
        if self.last_sequence <= 0 {
            return StreamRequest {
                query: None,
                last_event_id: None,
            };
        }
        StreamRequest {
            query: Some(format!("from_sequence_num={}", self.last_sequence)),
            last_event_id: Some(self.last_sequence.to_string()),
        }
    }

    /// Takes the status the service answered with. Any success status is read, whatever the content type; the
    /// statuses of [`PERMANENT_STREAM_STATUSES`] close the session with the status as the code, and every other
    /// status is retried. A stream that opens starts the reconnect delay over.
    pub fn opened(&mut self, status: u16) -> StreamOpen {
        if (200..300).contains(&status) {
            self.connected = true;
            self.attempts = 0;
            self.parser = FrameParser::default();
            return StreamOpen::Read;
        }
        if PERMANENT_STREAM_STATUSES.contains(&status) {
            return StreamOpen::Close(status);
        }
        StreamOpen::Retry
    }

    /// Takes a stream that ended, broke or never opened and says how long to wait before the next request, for a
    /// random draw in `[0, 1)`.
    pub fn lost(&mut self, random: f64) -> Duration {
        self.connected = false;
        self.attempts += 1;
        reconnect_delay(self.attempts, random)
    }

    /// Takes the expiry of the liveness timer (`onLivenessTimeout`). The silence is measured the way the upstream
    /// client measures it: the longer of the wall clock gap and the monotonic clock gap since the last connect or
    /// frame (`silentForMs`), so a clock that stopped while the process slept and one that jumped both show. A check
    /// that ran more than [`SLEEP_LATENESS_MS`] past [`liveness_timeout`] reopens the stream at once; any other
    /// expiry is a lost stream, counted twice (see below). The sleep notice the upstream transport can subscribe to is not reachable from
    /// `attachBridgeSession`, so this is the only way the reopen happens.
    pub fn liveness_expired(&mut self, wall_silent: Duration, monotonic_silent: Duration, random: f64) -> Reopen {
        self.liveness_expired_with(wall_silent, monotonic_silent, &mut || random)
    }

    /// [`liveness_expired`](Self::liveness_expired) for a caller that draws its random numbers as the upstream
    /// client does: none for a reopen at once, and one for each of the two lost-stream reports otherwise, so the
    /// draw order matches the recorded `Math.random` sequence. The second draw is the one that sets the wait.
    pub fn liveness_expired_with(
        &mut self,
        wall_silent: Duration,
        monotonic_silent: Duration,
        random: &mut dyn FnMut() -> f64,
    ) -> Reopen {
        let silent = wall_silent.max(monotonic_silent);
        let late = silent.saturating_sub(liveness_timeout());
        if self.connected && silent > liveness_timeout() + Duration::from_millis(SLEEP_LATENESS_MS) {
            self.connected = false;
            return Reopen::AtOnce { late };
        }
        // The liveness handler reports the lost stream, and the read loop its abort ends reports it again: two
        // attempts are counted and the second delay is the one that stands.
        let first = random();
        self.lost(first);
        let second = random();
        Reopen::After(self.lost(second))
    }

    /// Takes a chunk of the body and says what the frames in it ask for.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<StreamEffect> {
        self.push_counted(bytes).1
    }

    /// [`push`](Self::push), also saying how many frames the chunk completed: each one restarts the liveness timer,
    /// a chunk that ends inside a frame does not.
    pub fn push_counted(&mut self, bytes: &[u8]) -> (usize, Vec<StreamEffect>) {
        let frames = self.parser.push_bytes(bytes);
        let count = frames.len();
        let mut effects = Vec::new();
        for frame in frames {
            self.frame(frame, &mut effects);
        }
        (count, effects)
    }

    fn frame(&mut self, frame: SseFrame, effects: &mut Vec<StreamEffect>) {
        let event = frame.event.as_deref();
        if let Some(id) = frame.id.as_deref().filter(|id| !id.is_empty())
            && event != Some("ephemeral_event")
            && let Some(sequence) = js_parse_int(id)
        {
            self.note_sequence(sequence as i64);
        }
        let (Some(event), Some(data)) = (event, frame.data.as_deref()) else {
            return;
        };
        match event {
            "ephemeral_event" => ephemeral(data, effects),
            "client_event" => client_event(data, effects),
            _ => {}
        }
    }

    fn note_sequence(&mut self, sequence: i64) {
        if self.seen.insert(sequence) && self.seen.len() > SEEN_LIMIT {
            let floor = self.last_sequence.saturating_sub(SEEN_WINDOW);
            self.seen.retain(|seen| *seen >= floor);
        }
        self.last_sequence = self.last_sequence.max(sequence);
    }
}

fn ephemeral(data: &str, effects: &mut Vec<StreamEffect>) {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return;
    };
    if value.get("event_type").and_then(Value::as_str) == Some("heartbeat_probe") {
        effects.push(StreamEffect::HeartbeatProbe);
    }
}

fn client_event(data: &str, effects: &mut Vec<StreamEffect>) {
    let Ok(event) = serde_json::from_str::<Value>(data) else {
        return;
    };
    let payload = event.get("payload");
    let Some(payload_type) = payload.and_then(Value::as_object).and_then(|object| object.get("type")) else {
        acknowledge(&event, effects);
        return;
    };
    let server_only = payload_type
        .as_str()
        .is_some_and(|name| SERVER_ONLY_TYPES.contains(&name))
        && event.get("event_type") != Some(payload_type);
    let worker_control = payload_type.as_str() == Some("control_request")
        && event.get("source").and_then(Value::as_str) == Some("worker");
    if !server_only
        && !worker_control
        && let Some(payload) = payload
    {
        effects.push(StreamEffect::Deliver(format!("{payload}\n")));
    }
    acknowledge(&event, effects);
}

fn acknowledge(event: &Value, effects: &mut Vec<StreamEffect>) {
    if let Some(event_id) = event.get("event_id").and_then(Value::as_str) {
        effects.push(StreamEffect::Acknowledge {
            event_id: event_id.to_owned(),
        });
    }
}

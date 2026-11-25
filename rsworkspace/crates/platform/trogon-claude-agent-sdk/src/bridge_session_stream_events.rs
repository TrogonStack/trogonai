//! The window the upstream handle keeps stream events in before it uploads them (`bufferEphemeral`,
//! `takeStreamEventBuffer`).
//!
//! A stream event does not go out when it is written. The first one starts a window of 100 ms; the events written
//! meanwhile wait with it, runs of text, tool input and thinking deltas are merged (`HG`), and when the window ends
//! they leave together as live-view rows the store does not keep. An assistant row made of text only waits behind
//! them, so the deltas reach the viewer first. Any other row ends the window at once and goes out after what was
//! held. The session upload task owns the clock: [`StreamEvents`] only says when the window is due.

use serde_json::{Map, Value};
use tokio::time::Instant;

use crate::bridge_worker_events::ClientEvent;

mod constants;

use constants::{EPHEMERAL_LIMIT, LIVE_ONLY_LIMIT, MERGE_LIMIT, OVERSIZED_REPLY, WIDEST_ESCAPE, WINDOW};

type Frame = Map<String, Value>;

/// The events held back, the rows waiting behind them, and when the window ends.
#[derive(Default)]
pub(crate) struct StreamEvents {
    buffer: Vec<Frame>,
    parked: Vec<ClientEvent>,
    due: Option<Instant>,
}

impl StreamEvents {
    /// When the window ends; `None` while nothing is held.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.due
    }

    /// One frame the handle wrote at `now`: the events that go out because of it, in upload order.
    pub(crate) fn write(&mut self, frame: Value, now: Instant) -> Vec<ClientEvent> {
        let Value::Object(payload) = frame else {
            return Vec::new();
        };
        if payload.get("type").is_some_and(|kind| kind == "stream_event") {
            return self.hold(payload, now);
        }
        let waits = self.due.is_some() && is_text_row(&payload);
        let mut sent = if self.due.is_some() && !waits {
            self.take()
        } else {
            Vec::new()
        };
        let event = client_event(Value::Object(payload));
        if waits {
            self.parked.extend(event);
        } else {
            sent.extend(event);
        }
        sent
    }

    /// `bufferEphemeral`: keeps the event and starts the window when none is running. A block that starts while a row
    /// waits ends the window, so the row is not held up by what follows.
    fn hold(&mut self, mut payload: Frame, now: Instant) -> Vec<ClientEvent> {
        strip_transformations(&mut payload);
        let starts_block = event_type(&payload) == Some("content_block_start");
        self.buffer.push(payload);
        if self.due.is_none() {
            self.due = Some(now + WINDOW);
        }
        if starts_block && !self.parked.is_empty() {
            self.take()
        } else {
            Vec::new()
        }
    }

    /// `takeStreamEventBuffer`: ends the window. The held events come first, merged and without those too heavy to
    /// send, then the rows that waited.
    pub(crate) fn take(&mut self) -> Vec<ClientEvent> {
        self.due = None;
        let mut events: Vec<ClientEvent> = merge_deltas(std::mem::take(&mut self.buffer))
            .into_iter()
            .filter(|frame| serde_json::to_vec(frame).map_or(0, |bytes| bytes.len()) <= EPHEMERAL_LIMIT)
            .map(|payload| ClientEvent {
                payload,
                historical: false,
                ephemeral: true,
            })
            .collect();
        events.append(&mut self.parked);
        events
    }

    /// The handle closing: the held events are dropped and the waiting rows are handed back to be sent.
    pub(crate) fn close(&mut self) -> Vec<ClientEvent> {
        self.due = None;
        self.buffer.clear();
        std::mem::take(&mut self.parked)
    }
}

fn event_type(frame: &Frame) -> Option<&str> {
    frame.get("event")?.get("type")?.as_str()
}

/// `mayParkTextRow`: an assistant row whose content is text blocks only.
fn is_text_row(payload: &Frame) -> bool {
    payload.get("type").is_some_and(|kind| kind == "assistant")
        && payload
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                !blocks.is_empty()
                    && blocks
                        .iter()
                        .all(|block| block.get("type").is_some_and(|kind| kind == "text"))
            })
}

/// `X6`: the transformations of a message are not uploaded.
fn strip_transformations(payload: &mut Frame) {
    let Some(Value::Object(event)) = payload.get_mut("event") else {
        return;
    };
    let kind = event.get("type").and_then(Value::as_str).map(str::to_owned);
    match kind.as_deref() {
        Some("message_start") => {
            if let Some(Value::Object(message)) = event.get_mut("message")
                && message.get("input_transformations").is_some_and(truthy)
            {
                message.remove("input_transformations");
            }
        }
        Some("message_delta") if event.get("input_transformations").is_some_and(truthy) => {
            event.remove("input_transformations");
        }
        _ => {}
    }
}

/// JavaScript truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `BG`: a delta that carries text that may be joined to the one before it.
struct Delta {
    kind: &'static str,
    chunk: String,
    tokens: Option<f64>,
}

impl Delta {
    fn of(frame: &Frame) -> Option<Self> {
        let event = frame.get("event")?.as_object()?;
        if event.get("type")?.as_str()? != "content_block_delta" {
            return None;
        }
        let delta = event.get("delta")?.as_object()?;
        let (kind, key) = match delta.get("type")?.as_str()? {
            "text_delta" => ("text_delta", "text"),
            "input_json_delta" => ("input_json_delta", "partial_json"),
            "thinking_delta" => ("thinking_delta", "thinking"),
            _ => return None,
        };
        Some(Self {
            kind,
            chunk: delta.get(key)?.as_str()?.to_owned(),
            tokens: delta.get("estimated_tokens").and_then(Value::as_f64),
        })
    }
}

/// The deltas merged so far into the one that opened the run (`tA`).
struct Run {
    first: Frame,
    kind: &'static str,
    accumulated: String,
    tokens: Option<f64>,
    bytes: usize,
    count: usize,
}

impl Run {
    fn start(first: Frame, delta: Delta, bytes: usize) -> Self {
        Self {
            first,
            kind: delta.kind,
            accumulated: delta.chunk,
            tokens: delta.tokens,
            bytes,
            count: 1,
        }
    }

    /// `FG`: the same block, kind, parent and message, and still small enough.
    fn takes(&self, frame: &Frame, delta: &Delta, bytes: usize) -> bool {
        let block = |frame: &Frame| frame.get("event").and_then(|event| event.get("index")).cloned();
        self.kind == delta.kind
            && block(&self.first) == block(frame)
            && ["parent_tool_use_id", "thinking_display", "api_message_id"]
                .iter()
                .all(|key| self.first.get(*key) == frame.get(*key))
            && self.bytes + bytes <= MERGE_LIMIT
    }

    fn add(&mut self, delta: Delta, bytes: usize) {
        self.accumulated.push_str(&delta.chunk);
        self.bytes += bytes;
        self.count += 1;
        if let Some(tokens) = delta.tokens {
            self.tokens = Some(self.tokens.unwrap_or(0.0) + tokens);
        }
    }

    fn finish(self) -> Frame {
        let Self {
            mut first,
            kind,
            accumulated,
            tokens,
            count,
            ..
        } = self;
        if count > 1
            && let Some(Value::Object(event)) = first.get_mut("event")
            && let Some(Value::Object(delta)) = event.get_mut("delta")
        {
            let accumulated = Value::String(accumulated);
            match kind {
                "text_delta" => delta.insert("text".to_owned(), accumulated),
                "input_json_delta" => delta.insert("partial_json".to_owned(), accumulated),
                _ => {
                    delta.insert("thinking".to_owned(), accumulated);
                    delta.insert("estimated_tokens".to_owned(), tokens.map_or(Value::Null, number))
                }
            };
        }
        first
    }
}

/// A sum of token estimates as the number JavaScript would print: `3`, not `3.0`.
fn number(value: f64) -> Value {
    serde_json::from_str(&value.to_string()).unwrap_or(Value::Null)
}

/// `UG`: what a chunk weighs inside the quotes of a JSON string.
fn escaped_len(chunk: &str) -> usize {
    serde_json::to_string(chunk).map_or(chunk.len(), |text| text.len().saturating_sub(2))
}

/// `HG`: merges the runs of deltas, first without measuring, which is exact when the batch is small enough that
/// nothing could pass the limit, and otherwise by the bytes each chunk weighs once escaped.
fn merge_deltas(frames: Vec<Frame>) -> Vec<Frame> {
    if frames.len() <= 1 {
        return frames;
    }
    merge(&frames, false).unwrap_or_else(|| merge(&frames, true).unwrap_or_default())
}

/// `NG`: `None` when the batch holds too many characters to skip measuring.
fn merge(frames: &[Frame], measure: bool) -> Option<Vec<Frame>> {
    let mut merged = Vec::new();
    let mut run: Option<Run> = None;
    let mut characters = 0_usize;
    for frame in frames {
        let Some(delta) = Delta::of(frame) else {
            merged.extend(run.take().map(Run::finish));
            merged.push(frame.clone());
            continue;
        };
        let bytes = if measure {
            escaped_len(&delta.chunk)
        } else {
            characters += delta.chunk.encode_utf16().count();
            0
        };
        if let Some(open) = run.as_mut()
            && open.takes(frame, &delta, bytes)
        {
            open.add(delta, bytes);
            continue;
        }
        merged.extend(run.take().map(Run::finish));
        run = Some(Run::start(frame.clone(), delta, bytes));
    }
    merged.extend(run.take().map(Run::finish));
    (characters * WIDEST_ESCAPE <= MERGE_LIMIT).then_some(merged)
}

/// `toClientEvent`: the row as it is uploaded. Thinking-token rows and `live_only` control responses are live-view
/// rows. A live-only control response that would not fit is replaced by an error that says so (`rF`).
pub(crate) fn client_event(frame: Value) -> Option<ClientEvent> {
    let Value::Object(mut payload) = frame else {
        return None;
    };
    let live_only = payload.get("type").is_some_and(|kind| kind == "control_response")
        && payload.get("live_only").is_some_and(|flag| flag == true);
    if live_only && serde_json::to_string(&payload).map_or(0, |text| text.len()) > LIVE_ONLY_LIMIT {
        payload = oversized_reply(&payload);
    }
    let thinking = payload.get("type").is_some_and(|kind| kind == "system")
        && payload
            .get("subtype")
            .is_some_and(|subtype| subtype == "thinking_tokens");
    let historical = payload.get("historical").is_some_and(|flag| flag == true);
    Some(ClientEvent {
        payload,
        historical,
        ephemeral: live_only || thinking,
    })
}

fn oversized_reply(payload: &Map<String, Value>) -> Map<String, Value> {
    let mut replaced = Map::new();
    replaced.insert("type".to_owned(), Value::String("control_response".to_owned()));
    replaced.insert("live_only".to_owned(), Value::Bool(true));
    if let Some(uuid) = payload.get("uuid").filter(|uuid| uuid.is_string()) {
        replaced.insert("uuid".to_owned(), uuid.clone());
    }
    let mut response = Map::new();
    response.insert("subtype".to_owned(), Value::String("error".to_owned()));
    if let Some(id) = payload.get("response").and_then(|response| response.get("request_id")) {
        response.insert("request_id".to_owned(), id.clone());
    }
    response.insert("error".to_owned(), Value::String(OVERSIZED_REPLY.to_owned()));
    replaced.insert("response".to_owned(), Value::Object(response));
    replaced
}

#[cfg(test)]
#[path = "bridge_session_stream_events_tests.rs"]
mod tests;

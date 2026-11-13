//! The trust filter of the browser transport: what a parsed SSE frame becomes.

use serde_json::Value;

use crate::{
    JsonObject, SseCcrEvent, SseDeliveryUpdate, SseDropCategory, SseFrame, SseLogLevel, SseLogRecord, SsePreparedWrite,
    js_text::{
        JsConversionError, js_to_string, parse_json, positive_safe_integer, sanitize_log_text, sequence_number,
        sequence_number_text,
    },
    sse_drop_log::SseDropLog,
    sse_request_id_set::SseRequestIdSet,
    sse_worker_markers::{has_malformed_user_content, is_worker_output_user},
};

const ISSUED_CAP: usize = 500;
const EXPECTED_CAP: usize = 1024;
const PREFIX: &str = "[BrowserSSETransport]";
const STRIPPED_RESPONSE_KEYS: [&str; 4] = [
    "pending_user_dialog_requests",
    "pending_permission_requests",
    "pending_elicitation_requests",
    "error_code",
];

/// What the filter makes of a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    /// A message for the consumer, with the delivery envelope of a durable frame.
    Message {
        /// The payload, without the server's `ccr_event` bookkeeping.
        message: Value,
        /// Where the message sits in the server's event log; absent for ephemeral frames and for frames without
        /// an event id or sequence number.
        ccr_event: Option<SseCcrEvent>,
    },
    /// The server did not replay every event after the resume cursor.
    CatchUpTruncated,
    /// A delivery report for an event the client wrote.
    DeliveryUpdate(SseDeliveryUpdate),
    /// A line to log, in the order it arose among the other events.
    Log(SseLogRecord),
}

/// The pure half of the browser SSE transport: the resume cursor, the trust filter for incoming frames, and the
/// request-id bookkeeping for outgoing writes. It does no I/O; the transport feeds it frames and writes and acts
/// on what comes back.
#[derive(Debug)]
pub struct SseFrameFilter {
    last_sequence_num: u64,
    delivered_sequence_num: u64,
    issued: SseRequestIdSet,
    expected: SseRequestIdSet,
    drops: SseDropLog,
}

/// `tu`: `String(value)` sanitized for a log line, `undefined` standing for an absent value.
fn source_text(source: Option<&Value>) -> Result<String, JsConversionError> {
    Ok(sanitize_log_text(
        &source.map_or_else(|| Ok("undefined".to_owned()), js_to_string)?,
    ))
}

/// The keys of an ephemeral `system` status frame that survive, by subtype.
fn status_keys(subtype: &str) -> Option<&'static [&'static str]> {
    match subtype {
        "thinking_tokens" => Some(&["estimated_tokens", "estimated_tokens_delta", "user_message_uuid"]),
        "post_turn_summary" => Some(&["summarizes_uuid", "status_category", "status_detail", "needs_action"]),
        "task_summary" => Some(&["detail"]),
        "commands_changed" => Some(&["commands"]),
        _ => None,
    }
}

/// The display-only status message rebuilt from an ephemeral `system` payload, or `None` when it is not one the
/// transport accepts.
fn rebuild_status(payload: &JsonObject) -> Option<JsonObject> {
    let subtype = payload.get("subtype")?.as_str()?;
    if payload.get("type").and_then(Value::as_str) != Some("system") {
        return None;
    }
    let keys = status_keys(subtype)?;
    let mut status = JsonObject::new();
    status.insert("type".to_owned(), Value::String("system".to_owned()));
    status.insert("subtype".to_owned(), Value::String(subtype.to_owned()));
    for key in ["uuid", "session_id"].into_iter().chain(keys.iter().copied()) {
        if let Some(value) = payload.get(key) {
            status.insert(key.to_owned(), value.clone());
        }
    }
    Some(status)
}

impl SseFrameFilter {
    /// A filter that resumes after `from_sequence_num` when that is a positive safe integer.
    pub fn new(from_sequence_num: Option<f64>) -> Self {
        let seed = from_sequence_num.and_then(positive_safe_integer).unwrap_or(0);
        Self {
            last_sequence_num: seed,
            delivered_sequence_num: seed,
            issued: SseRequestIdSet::new("issuedRequestIds", ISSUED_CAP),
            expected: SseRequestIdSet::new("expectedControlResponseIds", EXPECTED_CAP),
            drops: SseDropLog::default(),
        }
    }

    /// The sequence number to send as `Last-Event-ID` when reconnecting, 0 for none.
    pub const fn resume_sequence_num(&self) -> u64 {
        self.last_sequence_num
    }

    /// The highest sequence number of a message the consumer has been given, if any.
    pub const fn last_sequence_num(&self) -> Option<u64> {
        if self.delivered_sequence_num > 0 {
            Some(self.delivered_sequence_num)
        } else {
            None
        }
    }

    /// Records that the consumer received a message with this envelope.
    pub fn mark_delivered(&mut self, ccr_event: Option<&SseCcrEvent>) {
        if let Some(event) = ccr_event.filter(|event| event.sequence_num > self.delivered_sequence_num) {
            self.delivered_sequence_num = event.sequence_num;
        }
    }

    /// How many frames were dropped per category, in the order the categories first occurred.
    pub fn drop_counts(&self) -> Vec<(SseDropCategory, u64)> {
        self.drops.counts()
    }

    /// The ids of the control requests this client wrote, oldest first.
    pub fn issued_request_ids(&self) -> impl Iterator<Item = &str> {
        self.issued.iter()
    }

    /// The ids of the control requests the client expects an answer to, oldest first.
    pub fn expected_request_ids(&self) -> impl Iterator<Item = &str> {
        self.expected.iter()
    }

    /// Notes that a control response for `request_id` is expected, so a non-worker frame carrying that id is
    /// refused. Returns the warning when the oldest expected id was evicted unanswered.
    pub fn expect_control_response(&mut self, request_id: &str) -> Option<SseLogRecord> {
        self.expected.insert(request_id, &self.issued)
    }

    /// Prepares `line` for writing: notes a control request's id and adds a `uuid` (from `new_uuid`) to a JSON
    /// object that has none.
    ///
    /// # Errors
    ///
    /// Returns the parse error when `line` is not JSON.
    pub fn prepare_write(
        &mut self,
        line: &str,
        new_uuid: impl FnOnce() -> String,
    ) -> Result<SsePreparedWrite, serde_json::Error> {
        let parsed = parse_json(line, "\\ufffd")?;
        let object = parsed.as_object();
        let log = object
            .filter(|object| object.get("type").and_then(Value::as_str) == Some("control_request"))
            .and_then(|object| object.get("request_id")?.as_str())
            .and_then(|id| self.issued.insert(id, &self.expected));
        Ok(SsePreparedWrite {
            line: SsePreparedWrite::with_uuid(line, object, new_uuid),
            log,
        })
    }

    fn advance_cursor(&mut self, sequence_num: Option<u64>) {
        if let Some(sequence_num) = sequence_num.filter(|sequence_num| *sequence_num > self.last_sequence_num) {
            self.last_sequence_num = sequence_num;
        }
    }

    fn drop_frame(&mut self, events: &mut Vec<SseEvent>, category: SseDropCategory, level: SseLogLevel, text: String) {
        events.extend(self.drops.record(category, level, text).map(SseEvent::Log));
    }

    /// Handles one frame from the parser and returns the events it produces, in order.
    pub fn handle_frame(&mut self, frame: &SseFrame) -> Vec<SseEvent> {
        let mut events = Vec::new();
        let kind = frame.event.as_deref();
        let cursor = if kind == Some("client_event") {
            frame.id.as_deref().and_then(sequence_number_text)
        } else {
            None
        };
        self.advance_cursor(cursor);
        let Some(data) = frame.data.as_deref().filter(|data| !data.is_empty()) else {
            return events;
        };
        match kind {
            Some("catch_up_truncated") => {
                let text = format!(
                    "{PREFIX} catch_up_truncated \u{2014} events after the resume cursor were not all replayed"
                );
                self.drop_frame(&mut events, SseDropCategory::CatchUpTruncated, SseLogLevel::Warn, text);
                events.push(SseEvent::CatchUpTruncated);
            }
            Some("delivery_update") => {
                match parse_json(data, "\\ufffd")
                    .ok()
                    .as_ref()
                    .and_then(SseDeliveryUpdate::from_value)
                {
                    Some(update) => events.push(SseEvent::DeliveryUpdate(update)),
                    None => {
                        let text = format!("{PREFIX} Dropping malformed delivery_update frame");
                        self.drop_frame(
                            &mut events,
                            SseDropCategory::MalformedDeliveryUpdate,
                            SseLogLevel::Debug,
                            text,
                        );
                    }
                }
            }
            Some(kind @ ("client_event" | "ephemeral_event")) => {
                if self.handle_payload(kind, cursor, data, &mut events).is_err() {
                    let text = format!("{PREFIX} Dropping unparseable {kind} frame");
                    self.drop_frame(&mut events, SseDropCategory::UnparseableFrame, SseLogLevel::Debug, text);
                }
            }
            _ => {}
        }
        events
    }

    /// The `client_event` and `ephemeral_event` frames. An error means the frame could not be handled.
    fn handle_payload(
        &mut self,
        kind: &str,
        mut cursor: Option<u64>,
        data: &str,
        events: &mut Vec<SseEvent>,
    ) -> Result<(), JsConversionError> {
        let parsed = match parse_json(data, "\\ufffd") {
            Ok(Value::Null) | Err(_) => return Err(JsConversionError),
            Ok(parsed) => parsed,
        };
        if kind == "client_event" && cursor.is_none() {
            cursor = sequence_number(parsed.get("sequence_num"))?;
            self.advance_cursor(cursor);
        }
        let (payload, root) = match parsed {
            Value::Object(mut root) => (root.shift_remove("payload"), root),
            _ => (None, JsonObject::new()),
        };
        let typed = payload.and_then(|payload| match payload {
            Value::Object(object) => Some((object.get("type")?.as_str()?.to_owned(), object)),
            _ => None,
        });
        let Some((payload_type, mut payload)) = typed else {
            let text = format!("{PREFIX} Dropping {kind} frame with no string payload.type");
            self.drop_frame(events, SseDropCategory::MissingType, SseLogLevel::Debug, text);
            return Ok(());
        };
        if kind == "ephemeral_event" {
            self.handle_ephemeral(&payload_type, payload, events);
            return Ok(());
        }
        let source = root.get("source");
        let from_worker = source.and_then(Value::as_str) == Some("worker");
        let envelope = cursor.and_then(|sequence_num| {
            let text = |key: &str| root.get(key).and_then(Value::as_str).map(str::to_owned);
            Some(SseCcrEvent {
                event_id: text("event_id")?,
                sequence_num,
                created_at: text("created_at"),
                source: text("source"),
            })
        });
        payload.shift_remove("ccr_event");
        let request_id = match payload_type.as_str() {
            "control_response" => {
                let id = payload
                    .get("response")
                    .and_then(Value::as_object)
                    .and_then(|response| response.get("request_id")?.as_str())
                    .map(str::to_owned);
                if id.is_none() {
                    let text = format!(
                        "{PREFIX} Dropping malformed control_response from source={}",
                        source_text(source)?
                    );
                    self.drop_frame(
                        events,
                        SseDropCategory::MalformedControlResponse,
                        SseLogLevel::Warn,
                        text,
                    );
                    return Ok(());
                }
                id
            }
            _ => None,
        };
        if payload_type == "user" {
            if !from_worker && is_worker_output_user(&payload) {
                let text = format!(
                    "{PREFIX} Dropping worker-output-shaped user frame from source={}",
                    source_text(source)?
                );
                self.drop_frame(events, SseDropCategory::UserWorkerOnly, SseLogLevel::Warn, text);
                return Ok(());
            }
            if has_malformed_user_content(&payload) {
                let text = format!(
                    "{PREFIX} Dropping user frame with malformed content from source={}",
                    source_text(source)?
                );
                self.drop_frame(events, SseDropCategory::MalformedUserContent, SseLogLevel::Warn, text);
                return Ok(());
            }
        }
        if let Some(request_id) = request_id {
            if from_worker {
                self.issued.remove(&request_id);
                self.expected.remove(&request_id);
            } else {
                if self.issued.contains(&request_id) || self.expected.contains(&request_id) {
                    let text = format!(
                        "{PREFIX} Dropping control_response for this client's request_id from source={}",
                        source_text(source)?
                    );
                    self.drop_frame(events, SseDropCategory::ForgedReply, SseLogLevel::Warn, text);
                    return Ok(());
                }
                if let Some(Value::Object(response)) = payload.get_mut("response") {
                    response.retain(|key, _| !STRIPPED_RESPONSE_KEYS.contains(&key.as_str()));
                }
            }
        } else if !from_worker && !matches!(payload_type.as_str(), "user" | "env_manager_log") {
            let echoed = matches!(payload_type.as_str(), "control_request" | "control_cancel_request")
                && payload
                    .get("request_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| self.issued.contains(id));
            let category = if echoed {
                SseDropCategory::ExpectedEcho
            } else {
                SseDropCategory::NonWorkerType
            };
            let text = format!(
                "{PREFIX} Dropping {} from source={}",
                sanitize_log_text(&payload_type),
                source_text(source)?
            );
            self.drop_frame(events, category, SseLogLevel::Debug, text);
            return Ok(());
        }
        events.push(SseEvent::Message {
            message: Value::Object(payload),
            ccr_event: envelope,
        });
        Ok(())
    }

    /// The `ephemeral_event` frames: streaming events, source changes and display-only status messages.
    fn handle_ephemeral(&mut self, payload_type: &str, mut payload: JsonObject, events: &mut Vec<SseEvent>) {
        let message = if matches!(payload_type, "stream_event" | "sources_changed") {
            payload.shift_remove("ccr_event");
            payload
        } else if let Some(status) = rebuild_status(&payload) {
            status
        } else {
            let label = match payload.get("subtype").and_then(Value::as_str) {
                Some(subtype) => format!("{payload_type}:{subtype}"),
                None => payload_type.to_owned(),
            };
            let text = format!(
                "{PREFIX} Dropping {} on ephemeral_event (only stream_event, sources_changed and display-only system status frames are accepted there)",
                sanitize_log_text(&label)
            );
            self.drop_frame(events, SseDropCategory::EphemeralType, SseLogLevel::Debug, text);
            return;
        };
        events.push(SseEvent::Message {
            message: Value::Object(message),
            ccr_event: None,
        });
    }
}

#[cfg(test)]
#[path = "sse_frame_filter_tests.rs"]
mod tests;

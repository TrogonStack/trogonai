//! The inbound router behind the read stream (`J4` in bridge.mjs, offset 805779): what each line the stream delivers
//! becomes, and which user messages are dropped before the host sees them.
//!
//! `attachBridgeSession` builds the router once per session with two bounded sets of user message uuids (`x9` = 2000
//! entries each): the echo set, filled by every frame the host writes (and by the undelivered frames a rebuilt
//! transport adopts), and the delivered set, filled by every user message handed on. A user message whose uuid is in
//! either set is dropped. The router does no I/O: [`InboundRouter::route`] says what the host side must do with a
//! line, and [`InboundRouter::remember_echo`] is the write path's half of the same rule.
//!
//! `attachBridgeSession` never passes the notification buffer, so a `queued_notification` the router cannot turn into
//! a user frame is logged and dropped. [`InboundRouter::new`] takes the flag the upstream router takes, for a caller
//! that has a buffer.

use std::collections::HashSet;

use serde_json::{Map, Value};

mod constants;

pub use constants::{INBOUND_DEDUP_CAPACITY, NOTIFICATION_ORIGIN};

/// A bounded set of strings that forgets the oldest one first (`If`). Adding a member it already has changes nothing.
#[derive(Clone, Debug)]
pub struct UuidSet {
    ring: Vec<Option<String>>,
    members: HashSet<String>,
    write_index: usize,
}

impl UuidSet {
    /// A set that remembers `capacity` members (at least one).
    pub fn new(capacity: usize) -> Self {
        Self {
            ring: vec![None; capacity.max(1)],
            members: HashSet::new(),
            write_index: 0,
        }
    }

    /// Adds a member and reports whether it was new. A full set drops its oldest member.
    pub fn add(&mut self, uuid: &str) -> bool {
        if self.members.contains(uuid) {
            return false;
        }
        if let Some(oldest) = self.ring[self.write_index].take() {
            self.members.remove(&oldest);
        }
        self.ring[self.write_index] = Some(uuid.to_owned());
        self.members.insert(uuid.to_owned());
        self.write_index = (self.write_index + 1) % self.ring.len();
        true
    }

    pub fn has(&self, uuid: &str) -> bool {
        self.members.contains(uuid)
    }

    pub fn clear(&mut self) {
        self.members.clear();
        self.ring.fill(None);
        self.write_index = 0;
    }
}

/// What the host side does with one inbound line.
#[derive(Clone, Debug, PartialEq)]
pub enum Inbound {
    /// A `control_response` (`onPermissionResponse`), with `requestId` normalized to `request_id`.
    ControlResponse(Value),
    /// A `control_request` for the control dispatch, with `requestId` normalized to `request_id`.
    ControlRequest(Value),
    /// A `control_cancel_request`: the request id whose cancellable work is aborted.
    ControlCancel(String),
    /// A `queued_notification` the router could not turn into a user frame, for a host with a notification buffer.
    QueuedNotification(Value),
    /// A user message for `onInboundMessage`.
    User(Value),
}

/// The routing and de-duplication state of one session's read stream.
#[derive(Clone, Debug)]
pub struct InboundRouter {
    echoes: UuidSet,
    delivered: UuidSet,
    notification_buffer: bool,
}

impl Default for InboundRouter {
    /// The router `attachBridgeSession` builds: no notification buffer.
    fn default() -> Self {
        Self::new(false)
    }
}

impl InboundRouter {
    pub fn new(notification_buffer: bool) -> Self {
        Self {
            echoes: UuidSet::new(INBOUND_DEDUP_CAPACITY),
            delivered: UuidSet::new(INBOUND_DEDUP_CAPACITY),
            notification_buffer,
        }
    }

    /// Records a uuid the host wrote, so the same message is not delivered back. Returns `false` when the uuid was
    /// already recorded: the write path (`write` on the handle) drops such a frame.
    pub fn remember_echo(&mut self, uuid: &str) -> bool {
        self.echoes.add(uuid)
    }

    pub fn is_echo(&self, uuid: &str) -> bool {
        self.echoes.has(uuid)
    }

    /// Decides what one line (a JSON payload) means. `None` is a line that is dropped: a parse failure, a message of
    /// no known kind, an echo, a message delivered before, a replay, a parented frame, a fork display frame, or a
    /// non-user message.
    pub fn route(&mut self, line: &str) -> Option<Inbound> {
        let mut message: Value = serde_json::from_str(line).ok()?;
        normalize_request_ids(&mut message);
        if is_control_response(&message) {
            return Some(Inbound::ControlResponse(message));
        }
        if is_control_request(&message) {
            return Some(Inbound::ControlRequest(message));
        }
        if has_type(&message)
            && message["type"] == "control_cancel_request"
            && let Some(request_id) = message.get("request_id").and_then(Value::as_str)
        {
            return Some(Inbound::ControlCancel(request_id.to_owned()));
        }
        if message.get("type").is_some_and(|kind| kind == "queued_notification") {
            let Some(user) = notification_as_user(message.get("notification")) else {
                if !self.notification_buffer {
                    return None;
                }
                let mut buffered = Map::new();
                buffered.insert("type".to_owned(), Value::String("queued_notification".to_owned()));
                if let Some(notification) = message.get("notification") {
                    buffered.insert("notification".to_owned(), notification.clone());
                }
                return Some(Inbound::QueuedNotification(Value::Object(buffered)));
            };
            message = user;
        }
        if !has_type(&message) {
            return None;
        }
        let uuid = message
            .get("uuid")
            .and_then(Value::as_str)
            .filter(|uuid| !uuid.is_empty())
            .map(str::to_owned);
        if let Some(uuid) = &uuid
            && (self.echoes.has(uuid) || self.delivered.has(uuid))
        {
            return None;
        }
        if message["type"] != "user" {
            return None;
        }
        if message.get("isReplay").is_some_and(|replay| replay == true) {
            return None;
        }
        if message
            .get("parent_tool_use_id")
            .is_some_and(|parent| !parent.is_null())
        {
            return None;
        }
        if is_fork_display_frame(&message) {
            return None;
        }
        if let Some(uuid) = &uuid {
            self.delivered.add(uuid);
        }
        Some(Inbound::User(message))
    }
}

/// `H4`: a camel-case `requestId` becomes `request_id` on the message and on its `response`, unless the id is a
/// placeholder a late-bound request carries (`late-bind-` in any case), which is dropped.
fn normalize_request_ids(message: &mut Value) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    normalize_request_id(object);
    if let Some(Value::Object(response)) = object.get_mut("response") {
        normalize_request_id(response);
    }
}

fn normalize_request_id(object: &mut Map<String, Value>) {
    if !object.contains_key("requestId") || object.contains_key("request_id") {
        return;
    }
    let id = object.shift_remove("requestId");
    let late_bound = matches!(&id, Some(Value::String(text)) if text.to_lowercase().starts_with("late-bind-"));
    if let (false, Some(id)) = (late_bound, id) {
        object.insert("request_id".to_owned(), id);
    }
}

/// `X4`: an object with a string `type`.
fn has_type(message: &Value) -> bool {
    message.get("type").is_some_and(Value::is_string)
}

/// `LPe`.
fn is_control_response(message: &Value) -> bool {
    message.get("type").is_some_and(|kind| kind == "control_response")
        && message.get("response").is_some_and(|response| !response.is_null())
}

/// `PPe`.
fn is_control_request(message: &Value) -> bool {
    message.get("type").is_some_and(|kind| kind == "control_request")
        && message.get("request_id").is_some()
        && message.get("request").is_some_and(|request| !request.is_null())
}

/// `Y4`: a recorded frame shown for display only.
fn is_fork_display_frame(message: &Value) -> bool {
    matches!(message["type"].as_str(), Some("user" | "assistant" | "system"))
        && message.get("historical").is_some_and(|flag| flag == true)
        && message.get("display_only").is_some_and(|flag| flag == true)
}

/// `XPe`: a queued notification of the one origin becomes the user frame it stands for.
fn notification_as_user(notification: Option<&Value>) -> Option<Value> {
    let notification = notification?.as_object()?;
    let id = notification
        .get("notification_id")?
        .as_str()
        .filter(|id| !id.is_empty())?;
    if notification.get("origin")? != NOTIFICATION_ORIGIN {
        return None;
    }
    let content = notification.get("content")?.as_str()?;
    let mut inner = Map::new();
    inner.insert("role".to_owned(), Value::String("user".to_owned()));
    inner.insert("content".to_owned(), Value::String(content.to_owned()));
    let mut user = Map::new();
    user.insert("type".to_owned(), Value::String("user".to_owned()));
    user.insert("uuid".to_owned(), Value::String(id.to_owned()));
    user.insert("message".to_owned(), Value::Object(inner));
    user.insert("parent_tool_use_id".to_owned(), Value::Null);
    user.insert("isSynthetic".to_owned(), Value::Bool(true));
    user.insert(
        "inbound_origin".to_owned(),
        Value::String(NOTIFICATION_ORIGIN.to_owned()),
    );
    Some(Value::Object(user))
}

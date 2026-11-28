//! The framed decider protocol over a byte stream pair, as a second
//! transport for the per-request decision beside the NATS exchange of
//! [`RequestDecider`](crate::request_decider::RequestDecider). Each frame is
//! a 4-byte big-endian length and one strict JSON object; the proxy speaks
//! first, the decider answers with its hello and then with one verdict per
//! request. Anything the protocol does not allow ends the exchange for good
//! and every request after that is refused.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
#[cfg(test)]
use base64::Engine as _;
#[cfg(test)]
use base64::engine::general_purpose::STANDARD;
use serde::de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Number, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use trogon_sandbox_contract::{LeaseId, RequestDecideResponse};

use crate::request_decider::{
    CREDENTIAL_HEADERS, CredentialPlacement, DeciderRules, FRAMING_HEADERS, MAX_CREDENTIAL, MAX_HEADER_BYTES,
    MAX_HEADER_ENTRIES, MAX_REASON_BYTES, MAX_REMOVE_NAMES, MAX_REQUEST_BODY, bad_request, is_token, is_value,
    unavailable,
};
use crate::request_decision::{DenyMark, RequestDenial, fold_header_name};
use crate::request_filter::{RequestDecision, RequestFilter, RequestView};

mod constants;
mod frame_encode;
use constants::{BODYLESS_METHOD_BODY_REASON, ID_WINDOW, MAX_ID, QUEUED_REQUEST_FRAME_ROOM};
pub use constants::{MAX_BODY_BYTES_HELD, MAX_READ_FRAME, PROTOCOL};
pub use frame_encode::{FrameEncodeError, encode_frame};
use frame_encode::{HelloOut, encode_body_frame};

/// What the decider's hello says about egress: the entries that are allowed
/// and the entries that are denied even when an allowed one matches.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct DeciderHello {
    pub allowed_domains: Vec<String>,
    pub denied_domains: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DeciderOptions {
    pub timeout: Duration,
    pub max_body_bytes_held: usize,
    pub max_queued_bytes: usize,
    pub placements: Vec<CredentialPlacement>,
}

impl Default for DeciderOptions {
    fn default() -> Self {
        Self::with_max_body_bytes_held(MAX_BODY_BYTES_HELD)
    }
}

impl DeciderOptions {
    pub fn with_max_body_bytes_held(held: usize) -> Self {
        Self {
            timeout: Duration::from_secs(5),
            max_body_bytes_held: held,
            max_queued_bytes: (held * 4).div_ceil(3) + QUEUED_REQUEST_FRAME_ROOM,
            placements: Vec::new(),
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn max_queued_bytes(mut self, queued: usize) -> Self {
        self.max_queued_bytes = queued;
        self
    }

    pub fn credential_placements(mut self, placements: Vec<CredentialPlacement>) -> Self {
        self.placements = placements;
        self
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum Late {
    Verdict,
    VerdictShown,
}

#[derive(Debug)]
enum Event {
    Verdict(Verdict),
    Dead(String),
}

#[derive(Debug)]
enum Verdict {
    Allow {
        set_headers: BTreeMap<String, Vec<String>>,
        remove_headers: Vec<String>,
        cred: Option<String>,
        credential: Option<String>,
    },
    Deny {
        status: u16,
        reason: Option<String>,
    },
    NeedBody {
        max: u64,
    },
}

#[derive(Default)]
struct State {
    dead: Option<String>,
    last_id: u64,
    waiters: HashMap<u64, oneshot::Sender<Event>>,
    late: HashMap<u64, Late>,
    held: usize,
    queued: usize,
    got_hello: bool,
    hello: Option<oneshot::Sender<DeciderHello>>,
}

struct Inner {
    rules: DeciderRules,
    options: DeciderOptions,
    state: Mutex<State>,
    frames: mpsc::UnboundedSender<Vec<u8>>,
    dead: watch::Sender<Option<String>>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn die(&self, why: impl Into<String>) {
        let why = why.into();
        let waiters = {
            let mut state = self.state();
            if state.dead.is_some() {
                return;
            }
            state.dead = Some(why.clone());
            state.hello = None;
            std::mem::take(&mut state.waiters)
        };
        for (_, waiter) in waiters {
            let _ = waiter.send(Event::Dead(why.clone()));
        }
        let _ = self.dead.send(Some(why));
    }

    fn send(&self, frame: Vec<u8>) {
        let queued = {
            let mut state = self.state();
            if state.dead.is_some() {
                return;
            }
            state.queued += frame.len();
            state.queued
        };
        if queued > self.options.max_queued_bytes {
            return self.die(format!("not reading: {queued} bytes queued"));
        }
        let _ = self.frames.send(frame);
    }

    fn on_frame(&self, frame: &[u8]) {
        let parsed = parse_strict(frame);
        let got_hello = self.state().got_hello;
        if !got_hello {
            let Some(hello) = parsed.and_then(|value| hello_of(value).ok()) else {
                return self.die("bad hello");
            };
            let mut state = self.state();
            state.got_hello = true;
            if let Some(tx) = state.hello.take() {
                let _ = tx.send(hello);
            }
            return;
        }
        let Some((id, verdict)) = parsed.and_then(|value| verdict_of(value).ok()) else {
            return self.die("bad verdict frame");
        };
        let mut state = self.state();
        if let Some(waiter) = state.waiters.remove(&id) {
            drop(state);
            let _ = waiter.send(Event::Verdict(verdict));
            return;
        }
        if let Some(late) = state.late.get(&id).copied() {
            let need_body = matches!(verdict, Verdict::NeedBody { .. });
            if need_body && late != Late::Verdict {
                drop(state);
                return self.die("need_body where none may come");
            }
            if !need_body {
                state.late.remove(&id);
            }
            return;
        }
        if id < state.last_id && state.last_id - id >= ID_WINDOW {
            return;
        }
        drop(state);
        self.die(format!("verdict for id {id}, which nothing waits on"));
    }

    fn prune_late(state: &mut State) {
        let last = state.last_id;
        state.late.retain(|id, _| last - id < ID_WINDOW);
    }
}

/// A [`RequestFilter`] that asks a decider over the framed protocol.
#[derive(Clone)]
pub struct FramedDecider {
    inner: Arc<Inner>,
}

impl FramedDecider {
    /// Sends the proxy's hello and waits for the decider's. Fails with the
    /// reason the exchange ended when the hello never comes.
    pub async fn open<R, W>(reader: R, writer: W, options: DeciderOptions) -> Result<(DeciderHello, Self), String>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (frames, queue) = mpsc::unbounded_channel();
        let (dead, mut dead_rx) = watch::channel(None);
        let (hello_tx, hello_rx) = oneshot::channel();
        let inner = Arc::new(Inner {
            rules: DeciderRules::with_placements(options.placements.clone()),
            options,
            state: Mutex::new(State {
                hello: Some(hello_tx),
                ..State::default()
            }),
            frames,
            dead,
        });
        let hello = encode_frame(&HelloOut {
            t: "hello",
            proto: PROTOCOL,
        })
        .map_err(|err| format!("decider unavailable: {err}"))?;
        inner.send(hello);
        tokio::spawn(write_loop(Arc::clone(&inner), writer, queue));
        tokio::spawn(read_loop(Arc::clone(&inner), reader));
        let hello = tokio::select! {
            hello = hello_rx => hello.ok(),
            _ = dead_rx.wait_for(Option::is_some) => None,
        };
        match hello {
            Some(hello) => Ok((hello, Self { inner })),
            None => {
                let why = inner.state().dead.clone().unwrap_or_default();
                Err(format!("decider unavailable: {why}"))
            }
        }
    }

    pub fn close(&self) {
        self.inner.die("closed");
    }

    /// Resolves with the reason once the exchange has ended.
    pub async fn closed(&self) -> String {
        let mut dead = self.inner.dead.subscribe();
        dead.wait_for(Option::is_some)
            .await
            .map(|why| why.clone().unwrap_or_default())
            .unwrap_or_default()
    }

    pub fn body_bytes_held(&self) -> usize {
        self.inner.state().held
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    inner: Arc<Inner>,
    mut writer: W,
    mut queue: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let mut dead = inner.dead.subscribe();
    loop {
        let frame = tokio::select! {
            frame = queue.recv() => frame,
            _ = dead.wait_for(Option::is_some) => None,
        };
        let Some(frame) = frame else { return };
        let written = async {
            writer.write_all(&frame).await?;
            writer.flush().await
        }
        .await;
        if let Err(err) = written {
            return inner.die(format!("write error: {err}"));
        }
        let mut state = inner.state();
        state.queued = state.queued.saturating_sub(frame.len());
    }
}

async fn read_loop<R: AsyncRead + Unpin>(inner: Arc<Inner>, mut reader: R) {
    let mut dead = inner.dead.subscribe();
    loop {
        let frame = tokio::select! {
            frame = read_frame(&mut reader) => frame,
            _ = dead.wait_for(Option::is_some) => return,
        };
        match frame {
            Ok(frame) => inner.on_frame(&frame),
            Err(why) => return inner.die(why),
        }
        if inner.state().dead.is_some() {
            return;
        }
    }
}

async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 4];
    read_exact(reader, &mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_READ_FRAME {
        return Err("oversized frame".to_string());
    }
    let mut frame = vec![0u8; length];
    read_exact(reader, &mut frame).await?;
    Ok(frame)
}

async fn read_exact<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut [u8]) -> Result<(), String> {
    match reader.read_exact(buf).await {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Err("decider closed the stream".to_string()),
        Err(err) => Err(format!("read error: {err}")),
    }
}

/// A JSON value that refuses what a lenient parser lets through: a `null`
/// anywhere and a duplicate key in any object.
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Walk;
        impl<'de> Visitor<'de> for Walk {
            type Value = Value;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON value without null")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
                Ok(Value::Bool(v))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
                Ok(Value::Number(v.into()))
            }

            fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
                Ok(Value::Number(v.into()))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
                Number::from_f64(v)
                    .map(Value::Number)
                    .ok_or_else(|| E::custom("a number JSON cannot hold"))
            }

            fn visit_str<E>(self, v: &str) -> Result<Value, E> {
                Ok(Value::String(v.to_string()))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
                Err(E::custom("null"))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
                let mut items = Vec::new();
                while let Some(Strict(item)) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Value::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
                let mut object = Map::new();
                while let Some((key, Strict(value))) = map.next_entry::<String, Strict>()? {
                    if object.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate key"));
                    }
                }
                Ok(Value::Object(object))
            }
        }
        deserializer.deserialize_any(Walk).map(Strict)
    }
}

/// One JSON object from a frame: valid UTF-8, one object and nothing else,
/// no `null`, no duplicate key.
pub fn parse_strict(frame: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(frame).ok()?;
    match serde_json::from_str::<Strict>(text).ok()?.0 {
        value @ Value::Object(_) => Some(value),
        _ => None,
    }
}

fn typed<T: DeserializeOwned>(value: Value) -> Result<T, ()> {
    serde_json::from_value(value).map_err(|_| ())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HelloFrame {
    t: String,
    proto: u64,
    allowed_domains: Vec<String>,
    denied_domains: Vec<String>,
}

fn unique(list: &[String]) -> bool {
    list.iter().collect::<BTreeSet<_>>().len() == list.len()
}

fn hello_of(value: Value) -> Result<DeciderHello, ()> {
    let hello: HelloFrame = typed(value)?;
    let fine = hello.t == "hello"
        && hello.proto == PROTOCOL
        && [&hello.allowed_domains, &hello.denied_domains]
            .iter()
            .all(|list| unique(list) && list.iter().all(|entry| is_egress_entry(entry)));
    if fine {
        Ok(DeciderHello {
            allowed_domains: hello.allowed_domains,
            denied_domains: hello.denied_domains,
        })
    } else {
        Err(())
    }
}

fn is_port(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 5
        && !text.starts_with('0')
        && text.bytes().all(|b| b.is_ascii_digit())
        && text.parse::<u32>().is_ok_and(|port| port <= 65535)
}

/// Whether `entry` belongs in the decider's egress lists: a lower-case DNS
/// name, the same after `*.` with two labels or more, or an IP literal, then
/// an optional `:port`. No scheme, path, user or space.
pub fn is_egress_entry(entry: &str) -> bool {
    if entry.parse::<IpAddr>().is_ok() {
        return true;
    }
    if let Some(rest) = entry.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        let (host, after) = rest.split_at(end);
        let after = &after[1..];
        let port_fine = match after {
            "" => true,
            _ => after.strip_prefix(':').is_some_and(is_port),
        };
        return host.parse::<std::net::Ipv6Addr>().is_ok() && port_fine;
    }
    let mut host = entry;
    if let Some(at) = entry.rfind(':') {
        host = &entry[..at];
        if !is_port(&entry[at + 1..]) {
            return false;
        }
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return ip.is_ipv4();
    }
    let wild = host.starts_with("*.");
    let base = if wild { &host[2..] } else { host };
    let labels: Vec<&str> = base.split('.').collect();
    if base.is_empty() || base.len() > 253 || (wild && labels.len() < 2) {
        return false;
    }
    if !labels.iter().all(|label| is_label(label)) {
        return false;
    }
    labels
        .last()
        .is_some_and(|last| last.starts_with(|c: char| c.is_ascii_lowercase()))
}

fn is_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    let edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !bytes.is_empty()
        && bytes.len() <= 63
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes.iter().all(|&b| edge(b) || b == b'-')
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AllowFrame {
    t: String,
    id: u64,
    action: String,
    set_headers: Option<BTreeMap<String, Vec<String>>>,
    remove_headers: Option<Vec<String>>,
    cred: Option<String>,
    credential: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DenyFrame {
    t: String,
    id: u64,
    action: String,
    status: u16,
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NeedBodyFrame {
    t: String,
    id: u64,
    action: String,
    max: u64,
}

fn lower_token(name: &str) -> bool {
    is_token(name) && name == name.to_ascii_lowercase()
}

fn verdict_of(value: Value) -> Result<(u64, Verdict), ()> {
    let action = value.get("action").and_then(Value::as_str).map(str::to_string);
    let valid_id = |id: u64| (1..=MAX_ID).contains(&id);
    match action.as_deref() {
        Some("allow") => {
            let frame: AllowFrame = typed(value)?;
            if frame.t != "verdict" || frame.action != "allow" || !valid_id(frame.id) {
                return Err(());
            }
            allow_of(
                frame.id,
                frame.set_headers,
                frame.remove_headers,
                frame.cred,
                frame.credential,
            )
        }
        Some("deny") => {
            let frame: DenyFrame = typed(value)?;
            let reason_fine = frame
                .reason
                .as_deref()
                .is_none_or(|reason| !reason.is_empty() && reason.len() <= MAX_REASON_BYTES && is_value(reason));
            if frame.t != "verdict"
                || frame.action != "deny"
                || !valid_id(frame.id)
                || !(400..=599).contains(&frame.status)
                || !reason_fine
            {
                return Err(());
            }
            Ok((
                frame.id,
                Verdict::Deny {
                    status: frame.status,
                    reason: frame.reason,
                },
            ))
        }
        Some("need_body") => {
            let frame: NeedBodyFrame = typed(value)?;
            if frame.t != "verdict"
                || frame.action != "need_body"
                || !valid_id(frame.id)
                || !(1..=MAX_REQUEST_BODY).contains(&frame.max)
            {
                return Err(());
            }
            Ok((frame.id, Verdict::NeedBody { max: frame.max }))
        }
        _ => Err(()),
    }
}

fn allow_of(
    id: u64,
    set_headers: Option<BTreeMap<String, Vec<String>>>,
    remove_headers: Option<Vec<String>>,
    cred: Option<String>,
    credential: Option<String>,
) -> Result<(u64, Verdict), ()> {
    if cred.is_some() != credential.is_some() {
        return Err(());
    }
    if let Some(cred) = &cred {
        let credential = credential.as_deref().unwrap_or_default();
        let printable = credential.bytes().all(|b| (0x21..=0x7e).contains(&b));
        if cred.is_empty() || credential.is_empty() || credential.len() > MAX_CREDENTIAL || !printable {
            return Err(());
        }
    }
    let never_set = |name: &str| {
        let folded = fold_header_name(name);
        CREDENTIAL_HEADERS.contains(&folded.as_str()) || FRAMING_HEADERS.contains(&folded.as_str())
    };
    let (mut entries, mut bytes) = (0, 0);
    if let Some(set) = &set_headers {
        if set.is_empty() {
            return Err(());
        }
        for (name, values) in set {
            if !lower_token(name) || never_set(name) || !values.iter().all(|v| is_value(v)) {
                return Err(());
            }
            entries += values.len().max(1);
            bytes += name.len() + values.iter().map(String::len).sum::<usize>();
            if entries > MAX_HEADER_ENTRIES || bytes > MAX_HEADER_BYTES {
                return Err(());
            }
        }
    }
    if let Some(remove) = &remove_headers {
        let framing = |name: &str| FRAMING_HEADERS.contains(&fold_header_name(name).as_str());
        if remove.is_empty()
            || !unique(remove)
            || remove.len() > MAX_REMOVE_NAMES
            || remove.iter().map(String::len).sum::<usize>() > MAX_HEADER_BYTES
            || !remove.iter().all(|name| lower_token(name) && !framing(name))
        {
            return Err(());
        }
    }
    Ok((
        id,
        Verdict::Allow {
            set_headers: set_headers
                .unwrap_or_default()
                .into_iter()
                .filter(|(_, values)| !values.is_empty())
                .collect(),
            remove_headers: remove_headers.unwrap_or_default(),
            cred,
            credential,
        },
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReqFrame<'a> {
    t: &'static str,
    id: u64,
    method: &'a str,
    host: &'a str,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_header: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sni: Option<&'a str>,
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<&'a str>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    headers: &'a BTreeMap<String, Vec<String>>,
}

enum Wait {
    Verdict(Verdict),
    Dead(String),
    TimedOut,
}

/// Records the id as late when a request ends while its verdict is still
/// awaited, so the verdict that comes afterwards is dropped, not fatal.
struct Pending<'a> {
    inner: &'a Inner,
    id: u64,
    late: Late,
    armed: bool,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut state = self.inner.state();
        if state.waiters.remove(&self.id).is_some() {
            state.late.insert(self.id, self.late);
        }
    }
}

struct Reservation<'a> {
    inner: &'a Inner,
    bytes: usize,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut state = self.inner.state();
        state.held = state.held.saturating_sub(self.bytes);
    }
}

fn declares_a_body(request: &RequestView<'_>) -> bool {
    let header = |wanted: &str| {
        request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
            .map(|(_, value)| value.trim())
    };
    header("transfer-encoding").is_none()
        && header("content-length").is_some_and(|length| length.parse::<u64>().is_ok_and(|n| n > 0))
}

fn framed_unavailable(why: &str) -> RequestDecision {
    unavailable(&format!("request decider: {why}"))
}

fn broke_protocol() -> RequestDecision {
    framed_unavailable("decider broke the protocol")
}

impl FramedDecider {
    fn register(&self, id: u64) -> oneshot::Receiver<Event> {
        let (tx, rx) = oneshot::channel();
        let mut state = self.inner.state();
        match &state.dead {
            Some(why) => {
                let _ = tx.send(Event::Dead(why.clone()));
            }
            None => {
                state.waiters.insert(id, tx);
            }
        }
        rx
    }

    async fn wait(&self, id: u64, rx: &mut oneshot::Receiver<Event>, late: Late) -> Wait {
        let mut pending = Pending {
            inner: &self.inner,
            id,
            late,
            armed: true,
        };
        let outcome = tokio::time::timeout(self.inner.options.timeout, &mut *rx).await;
        pending.armed = false;
        let event = match outcome {
            Ok(event) => event.ok(),
            Err(_) => {
                let mut state = self.inner.state();
                if state.waiters.remove(&id).is_some() {
                    state.late.insert(id, late);
                    Inner::prune_late(&mut state);
                    return Wait::TimedOut;
                }
                drop(state);
                rx.try_recv().ok()
            }
        };
        match event {
            Some(Event::Verdict(verdict)) => Wait::Verdict(verdict),
            Some(Event::Dead(why)) => Wait::Dead(why),
            None => Wait::Dead(self.inner.state().dead.clone().unwrap_or_default()),
        }
    }

    fn settle(wait: Wait) -> Result<Verdict, RequestDecision> {
        match wait {
            Wait::Verdict(verdict) => Ok(verdict),
            Wait::Dead(why) => Err(framed_unavailable(&why)),
            Wait::TimedOut => Err(framed_unavailable("decider timed out")),
        }
    }

    fn decision(&self, verdict: Verdict, host: &str) -> RequestDecision {
        let reply = match verdict {
            Verdict::Allow {
                set_headers,
                remove_headers,
                cred,
                credential,
            } => RequestDecideResponse::Allow {
                set_headers,
                remove_headers,
                cred,
                credential,
            },
            Verdict::Deny { status, reason } => RequestDecideResponse::Deny { status, reason },
            Verdict::NeedBody { max } => RequestDecideResponse::NeedBody { max },
        };
        self.inner.rules.decision(reply, host)
    }

    fn next_id(&self) -> Option<u64> {
        let mut state = self.inner.state();
        let id = state.last_id + 1;
        if id > MAX_ID {
            return None;
        }
        state.last_id = id;
        Some(id)
    }

    async fn ask(&self, request: &RequestView<'_>) -> RequestDecision {
        let dead = self.inner.state().dead.is_some();
        if dead {
            return unavailable("request decider unavailable");
        }
        let question = match self.inner.rules.question(&LeaseId::new(String::new()), request) {
            Ok(question) => question,
            Err(reason) => return bad_request(reason),
        };
        let host = question.host.clone();
        let build = |id: u64| ReqFrame {
            t: "req",
            id,
            method: &question.method,
            host: &question.host,
            port: question.port,
            host_header: question.host_header.as_deref(),
            sni: question.sni.as_deref(),
            path: &question.path,
            query: question.query.as_deref(),
            headers: &question.headers,
        };
        match encode_frame(&build(MAX_ID)) {
            Ok(frame) if frame.len() - 4 <= MAX_READ_FRAME => {}
            Ok(_) => return bad_request("request too large for the decider"),
            Err(err) => return bad_request(&err.to_string()),
        }
        let Some(id) = self.next_id() else {
            self.inner.die("request ids exhausted");
            return unavailable("request decider unavailable");
        };
        let frame = match encode_frame(&build(id)) {
            Ok(frame) => frame,
            Err(err) => return bad_request(&err.to_string()),
        };
        let mut rx = self.register(id);
        self.inner.send(frame);
        let first = self.wait(id, &mut rx, Late::Verdict).await;
        let verdict = match Self::settle(first) {
            Ok(verdict) => verdict,
            Err(decision) => return decision,
        };
        let Verdict::NeedBody { max } = verdict else {
            return self.decision(verdict, &host);
        };
        self.show_body(id, max, request, &host).await
    }

    async fn show_body(&self, id: u64, max: u64, request: &RequestView<'_>, host: &str) -> RequestDecision {
        let max = usize::try_from(max).unwrap_or(usize::MAX);
        let bodyless = matches!(request.method.as_str(), "GET" | "HEAD" | "OPTIONS");
        if bodyless && let Some(body) = request.body {
            self.mark_late(id, Late::VerdictShown);
            let declared = declares_a_body(request);
            let seen = if declared {
                Ok(true)
            } else {
                body.read(1).await.map(|read| !read.bytes.is_empty())
            };
            match seen {
                Ok(true) => {
                    let denial = RequestDenial::new().status(403).reason(BODYLESS_METHOD_BODY_REASON);
                    let marked = DenyMark::parse("bodyless_method_body")
                        .map(|mark| denial.clone().mark(mark))
                        .unwrap_or(denial);
                    return RequestDecision::DenyWith(marked);
                }
                Ok(false) => {}
                Err(_) => return framed_unavailable("the request body is not available"),
            }
            if self.state_late_removed(id).is_none() {
                self.inner.die("answered before it was sent the body");
                return framed_unavailable("the decider answered before it was sent the body");
            }
        }
        let wanted = if bodyless {
            0
        } else {
            let length = request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok());
            length.map_or(max, |length| length.min(max))
        };
        let reservation = {
            let mut state = self.inner.state();
            if state.held + wanted > self.inner.options.max_body_bytes_held {
                state.late.insert(id, Late::VerdictShown);
                drop(state);
                return framed_unavailable("too many request bodies held for inspection");
            }
            state.held += wanted;
            Reservation {
                inner: &self.inner,
                bytes: wanted,
            }
        };
        let mut during = self.register(id);
        let mut pending = Pending {
            inner: &self.inner,
            id,
            late: Late::VerdictShown,
            armed: true,
        };
        let read = async {
            match (bodyless, request.body) {
                (true, _) => Ok(crate::request_body::BodyRead {
                    bytes: Vec::new(),
                    cut: false,
                }),
                (false, Some(body)) => body.read(max).await.map_err(|_| ()),
                (false, None) => Err(()),
            }
        };
        let read = tokio::select! {
            read = read => Ok(read),
            event = &mut during => Err(event),
        };
        let read = match read {
            Ok(read) => read,
            Err(event) => {
                pending.armed = false;
                return self.during_read(event.ok(), host);
            }
        };
        let Ok(read) = read else {
            return framed_unavailable("the request body is not available");
        };
        let swapped = {
            let mut state = self.inner.state();
            if let Some(why) = &state.dead {
                return framed_unavailable(why);
            }
            if state.waiters.remove(&id).is_some() {
                let (tx, rx) = oneshot::channel();
                state.waiters.insert(id, tx);
                Some(rx)
            } else {
                None
            }
        };
        let Some(mut next) = swapped else {
            pending.armed = false;
            return self.during_read(during.try_recv().ok(), host);
        };
        self.inner.send(encode_body_frame(id, &read.bytes, read.cut));
        pending.armed = false;
        let after = self.wait(id, &mut next, Late::VerdictShown).await;
        drop(reservation);
        match Self::settle(after) {
            Ok(Verdict::NeedBody { .. }) => {
                self.inner.die("a second need_body");
                broke_protocol()
            }
            Ok(verdict) => self.decision(verdict, host),
            Err(decision) => decision,
        }
    }

    fn during_read(&self, event: Option<Event>, host: &str) -> RequestDecision {
        match event {
            Some(Event::Verdict(verdict @ Verdict::Deny { .. })) => self.decision(verdict, host),
            Some(Event::Verdict(_)) => {
                self.inner.die("a verdict other than deny while the body was read");
                broke_protocol()
            }
            Some(Event::Dead(why)) => framed_unavailable(&why),
            None => framed_unavailable(&self.inner.state().dead.clone().unwrap_or_default()),
        }
    }

    fn mark_late(&self, id: u64, late: Late) {
        self.inner.state().late.insert(id, late);
    }

    fn state_late_removed(&self, id: u64) -> Option<Late> {
        self.inner.state().late.remove(&id)
    }
}

#[async_trait]
impl RequestFilter for FramedDecider {
    async fn decide(&self, request: &RequestView<'_>) -> RequestDecision {
        self.ask(request).await
    }
}

#[cfg(test)]
mod tests;

//! The worker half of the bridge: registering a worker for a remote session and keeping the service told about it
//! (`PUT /worker` state and metadata, `POST /worker/heartbeat`), with the request path rules the upstream bridge
//! entrypoint applies to every answer, and the client event upload (`POST /worker/events`, [`crate::EventQueue`]).
//!
//! Not ported yet, and reachable: the events a flush policy holds or merges, stream-event buffering, the control
//! request urgent lane, rider and receipt uploads (`/worker/event-batch`), delivery acknowledgements and the internal
//! event uploader. The event queue takes durable rows and `thinking_tokens` rows only until those land.
//!
//! Draft, pending upstream call-graph evidence for each entry below. The port covers what a caller reaches through
//! `attachBridgeSession` and the handle it returns, and what a service answer can trigger. These branches of the
//! upstream client look out of reach there and are not ported yet:
//!
//! - the worker socket (`workerSocket`, `O?"origin"`, offsets 1051556 to 1052200), which no attach option creates;
//! - the heartbeat probe and the token-lifetime extras (`advertiseHeartbeatProbeSupport`, `skipRedundantHeartbeats`,
//!   offset 1048000), which the attach options leave off;
//! - the flush policy (`flushPolicy`: `waitsForStatus`, `mergesSameRun`, `metadataNeverSent`, offsets 1032337 and
//!   1057745), which is only set by a policy the attach options never deliver;
//! - `e3()` for `reply_degraded` (offset 856241), which evaluates to false for an attached session;
//! - the init metadata `f` in `recordAtStart` (offsets 1029258 and 1045899): `initExternalMetadata` and
//!   `metadataNeverSent` are unset, so `f` is always empty;
//! - `reportInternalMetadata` and the `disabled_mcp_servers` refused-alone bookkeeping (offsets 1030099 to 1031477,
//!   1128273 to 1128840): the attach handle exposes `reportState`, `reportMetadata` and `reportDelivery` only, so no
//!   patch carries `disabled_mcp_servers`;
//! - `onConnectNeedsRecord` (offset 1196900): never set, so the stored record read is abandoned at registration.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Method;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Map, Value, json};

use crate::SensitiveString;
use crate::adapters::HttpExecute;
use crate::bridge_client::{USER_AGENT, client_platform, header_text};
use crate::bridge_js::{js_number, js_parse_int, plain_detail, safe_integer};
use crate::bridge_types::SessionState;
use crate::bridge_worker_events::{ClientEvent, EventNext, EventQueue, refused_for_good, upload_body, upload_timeout};
use crate::bridge_worker_queue::{Next, Patch, PatchQueue, requeue_after_refusal, without_status};
use crate::bridge_worker_types::{
    InitError, InitFailure, MismatchReason, RegisterFailure, RequestFailure, RequestOutcome, RequiresActionDetails,
};
use crate::settings_json::js_stringify;
use crate::settings_parse::parse_json;

/// How often the worker beats until the service advises another interval.
pub const DEFAULT_HEARTBEAT_INTERVAL_MS: u64 = 20_000;
/// The shortest interval the service may advise.
pub const HEARTBEAT_INTERVAL_FLOOR_MS: u64 = 10_000;
/// The longest interval the service may advise.
pub const HEARTBEAT_INTERVAL_CEILING_MS: u64 = 300_000;
/// The longest a single heartbeat may wait for its answer.
pub const HEARTBEAT_TIMEOUT_CEILING_MS: u64 = 15_000;
/// How much of the interval a heartbeat leaves for the next one to start on time.
pub const HEARTBEAT_TIMEOUT_MARGIN_MS: u64 = 5_000;
/// The shortest a single heartbeat may wait for its answer.
pub const HEARTBEAT_TIMEOUT_FLOOR_MS: u64 = 5_000;
/// Consecutive `401` or `403` answers after which the worker gives up.
pub const AUTH_FAILURES_BEFORE_EXIT: u32 = 10;
/// Consecutive `404` answers after which the worker gives up.
pub const NOT_FOUND_BEFORE_EXIT: u32 = 3;

const REGISTER_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const INIT_ATTEMPTS: u32 = 10;
const GET_ATTEMPTS: u32 = 10;
const GET_TIMEOUT: Duration = Duration::from_secs(30);
/// `initStateGetOrderingBoundMs` (bridge.mjs offset 1133 region, `dHe`): how long `initialize` waits for the stored
/// record before it registers.
const INIT_STATE_GET_ORDERING_BOUND: Duration = Duration::from_secs(10);
const INIT_BACKOFF_CEILING_MS: u64 = 30_000;
/// `qHe` (bridge.mjs offset 1019297): the metadata keys that are not sent again when the stored record holds them.
const RECORDED_KEYS: [&str; 6] = [
    "context_usage",
    "cross_session_inbound",
    "is_ultraplan_mode",
    "last_served_model",
    "permission_mode",
    "rate_limit_info",
];
const SESSION_TICKET_HEADER: &str = "x-ccr-ticket-session";
const TICKET_HEADERS: [&str; 3] = ["x-ccr-ticket-rev", SESSION_TICKET_HEADER, "x-ccr-ticket-seq"];

/// What the worker waits with: the pause between init attempts and the jitter added to it.
pub trait WorkerPacing {
    /// A number in `[0, 1)`, the stand-in for `Math.random()`.
    fn jitter(&self) -> f64;
    fn pause(&self, delay: Duration) -> impl Future<Output = ()> + Send;
}

/// Pauses on the Tokio timer and takes its jitter from the clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioPacing;

impl WorkerPacing for TokioPacing {
    fn jitter(&self) -> f64 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        f64::from(nanos % 1000) / 1000.0
    }

    fn pause(&self, delay: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(delay)
    }
}

/// A session id the service would not accept in a URL.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Invalid sessionId: contains unsafe characters")]
pub struct InvalidSessionId;

/// `${apiBaseUrl without trailing slashes}/v1/code/sessions/${sessionId}`, for a session id of letters, digits,
/// `_` and `-`.
pub fn session_url(api_base_url: &str, session_id: &str) -> Result<String, InvalidSessionId> {
    let safe = !session_id.is_empty()
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !safe {
        return Err(InvalidSessionId);
    }
    Ok(format!(
        "{}/v1/code/sessions/{session_id}",
        api_base_url.trim_end_matches('/')
    ))
}

/// The wait one heartbeat gets for its answer when the beat interval is `interval_ms`.
pub fn heartbeat_timeout(interval_ms: f64) -> Duration {
    let floor = HEARTBEAT_TIMEOUT_FLOOR_MS as f64;
    let ceiling = HEARTBEAT_TIMEOUT_CEILING_MS as f64;
    let wait = (interval_ms - HEARTBEAT_TIMEOUT_MARGIN_MS as f64).clamp(floor, ceiling);
    Duration::from_secs_f64(wait / 1000.0)
}

/// The `external_metadata` the service gets for `metadata`: a `blocked` post-turn summary goes out as `need_input`.
pub fn external_metadata_patch(metadata: &Map<String, Value>) -> Map<String, Value> {
    let mut patch = metadata.clone();
    if let Some(Value::Object(summary)) = patch.get_mut("post_turn_summary")
        && summary.get("status_category").and_then(Value::as_str) == Some("blocked")
    {
        summary.insert("status_category".into(), Value::String("need_input".into()));
    }
    patch
}

/// How a refused request reads to a caller that only needs to know whether the service saw it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusClass {
    /// The service may or may not have applied the request.
    Indeterminate,
    /// The service answered with a client error and did not apply the request.
    NotAccepted,
}

/// `d5`: a `4xx` status is a definite refusal; no status, or any other one, leaves the outcome open.
pub fn classify_status(status: Option<u16>) -> StatusClass {
    match status {
        Some(400..=499) => StatusClass::NotAccepted,
        _ => StatusClass::Indeterminate,
    }
}

/// The wait a `429` asks for in its `retry-after` header: whole seconds read like `parseInt(value, 10)`, kept when
/// they are not negative.
pub fn retry_after_ms(header: &str) -> Option<f64> {
    js_parse_int(header)
        .filter(|seconds| *seconds >= 0.0)
        .map(|seconds| seconds * 1000.0)
}

/// What the worker needs to know about its session.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub api_base_url: String,
    pub session_id: String,
    pub token: SensitiveString,
    /// The `CLAUDE_CODE_ENTRYPOINT` value, which picks the `anthropic-client-platform` header.
    pub entrypoint: Option<String>,
    /// The first beat interval; the upstream default when absent.
    pub heartbeat_interval_ms: Option<u64>,
    /// The `CLAUDE_CODE_SESSION_ACCESS_TOKEN` value, which a `401` or `403` is checked against for an expired `exp`.
    pub session_access_token: Option<SensitiveString>,
    /// What a `403` that did not come from the service counts as.
    pub non_origin_403: NonOrigin403,
}

/// What a `403` that the service did not send (`request-id` absent or malformed) counts as: `nonOrigin403Transient`.
/// The attach entrypoint leaves it unset, which reads as `false` (bridge.mjs offsets 1027202 and 1054671), so
/// `Counted` is the default and matches the recorded goldens. `Transient` is the upstream `true` form of the option,
/// which the attach entrypoint cannot be made to set, so it has no recorded golden and is covered by a test of this
/// client alone. The object form (windows and the `onRefused` callbacks, offsets 1054671 to 1057168) needs the
/// upload lanes and their backlog, which the attach entrypoint never configures, so it is not part of this client.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NonOrigin403 {
    /// The refusal counts like any other `401` or `403`.
    #[default]
    Counted,
    /// The refusal is transient: it is flagged and does not count toward the auth failures that end the worker.
    Transient,
}

/// Who a refused request came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefusalSource {
    /// The service itself: the answer carries a well-formed `request-id`.
    Origin,
    /// An edge network: `cf-ray` is set or `server` names Cloudflare.
    NonOriginCloudflare,
    NonOriginOther,
}

impl RefusalSource {
    /// The name the upstream request path logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Origin => "origin",
            Self::NonOriginCloudflare => "nonorigin_cf",
            Self::NonOriginOther => "nonorigin_other",
        }
    }

    pub fn is_non_origin(self) -> bool {
        self != Self::Origin
    }
}

fn valid_request_id(text: &str) -> bool {
    text.strip_prefix("req_").is_some_and(|rest| {
        (1..=36).contains(&rest.len()) && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

/// `Us`: where an answer came from, read from its headers.
pub fn refusal_source(headers: &HeaderMap) -> RefusalSource {
    if header_text(headers, "request-id").is_some_and(|id| valid_request_id(id.trim())) {
        return RefusalSource::Origin;
    }
    let ray = header_text(headers, "cf-ray").is_some_and(|ray| !ray.is_empty());
    let edge = header_text(headers, "server").is_some_and(|server| server.to_lowercase().contains("cloudflare"));
    if ray || edge {
        RefusalSource::NonOriginCloudflare
    } else {
        RefusalSource::NonOriginOther
    }
}

/// What `Buffer.from(text, "base64url")` reads: both alphabets, nothing for characters outside them, and the end at
/// the first `=`. Bits that do not make a whole byte are dropped.
fn node_base64url(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let (mut bits, mut count) = (0_u32, 0_u32);
    for c in text.chars() {
        let value = match c {
            'A'..='Z' => u32::from(c) - u32::from('A'),
            'a'..='z' => u32::from(c) - u32::from('a') + 26,
            '0'..='9' => u32::from(c) - u32::from('0') + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => continue,
        };
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push(((bits >> count) & 0xff) as u8);
            bits &= (1 << count) - 1;
        }
    }
    bytes
}

/// `eA`: the `exp` claim, in seconds, of a session token (a JWT, optionally behind the `sk-ant-si-` prefix).
pub fn token_expiry(token: &str) -> Option<f64> {
    let token = token.strip_prefix("sk-ant-si-").unwrap_or(token);
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 || parts[1].is_empty() {
        return None;
    }
    let payload = String::from_utf8_lossy(&node_base64url(parts[1])).into_owned();
    match parse_json(&payload).ok()? {
        Value::Object(claims) => match claims.get("exp") {
            Some(Value::Number(exp)) => exp.as_f64(),
            _ => None,
        },
        _ => None,
    }
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_millis() as f64)
}

/// The header value for `text` as Node writes it: one byte per character, which must fit in a byte. `fetch` trims
/// the whitespace around a value; the HTTP client `registerWorker` uses drops line breaks instead.
fn header_value(text: &str, for_fetch: bool) -> Option<HeaderValue> {
    let mut bytes = Vec::with_capacity(text.len());
    for c in text.chars() {
        if !for_fetch && (c == '\r' || c == '\n') {
            continue;
        }
        bytes.push(u8::try_from(u32::from(c)).ok()?);
    }
    if for_fetch {
        let space = |byte: &u8| matches!(byte, b'\t' | b'\n' | b'\r' | b' ');
        let start = bytes.iter().position(|byte| !space(byte)).unwrap_or(bytes.len());
        let end = bytes.iter().rposition(|byte| !space(byte)).map_or(start, |at| at + 1);
        bytes = bytes[start..end].to_vec();
    }
    let mut value = HeaderValue::from_bytes(&bytes).ok()?;
    value.set_sensitive(true);
    Some(value)
}

/// What axios throws for a request that never got an answer, read from the transport error.
fn network_failure(url: &reqwest::Url, error: &reqwest::Error) -> RegisterFailure {
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_matches(|c| c == '[' || c == ']');
    let port = url.port_or_known_default().unwrap_or_default();
    let named = |message: String, code: &str| RegisterFailure::Network {
        message,
        code: Some(code.to_owned()),
    };
    if error.is_redirect() {
        return named(
            "Maximum number of redirects exceeded".to_owned(),
            "ERR_FR_TOO_MANY_REDIRECTS",
        );
    }
    if error.is_body() || error.is_decode() {
        return named("stream has been aborted".to_owned(), "ERR_BAD_RESPONSE");
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        let shown = current.to_string();
        if shown.contains("dns error") || shown.contains("failed to lookup address information") {
            return named(format!("getaddrinfo ENOTFOUND {host}"), "ENOTFOUND");
        }
        if shown.contains("connection closed before message completed") {
            return named("socket hang up".to_owned(), "ECONNRESET");
        }
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            match io.kind() {
                std::io::ErrorKind::ConnectionRefused if host.eq_ignore_ascii_case("localhost") => {
                    // `localhost` names both loopback addresses, and when every one refuses node raises an
                    // `AggregateError`, which has no message of its own.
                    return named(String::new(), "ECONNREFUSED");
                }
                std::io::ErrorKind::ConnectionRefused => {
                    return named(format!("connect ECONNREFUSED {host}:{port}"), "ECONNREFUSED");
                }
                std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::UnexpectedEof => return named("socket hang up".to_owned(), "ECONNRESET"),
                _ => {}
            }
        }
        source = current.source();
    }
    named("Network Error".to_owned(), "ERR_NETWORK")
}

enum Fault {
    TimedOut,
    Unreachable(Box<reqwest::Error>),
}

struct Reply {
    status: u16,
    headers: HeaderMap,
    /// The whole body, or nothing when reading it failed or ran out of time.
    body: Option<Vec<u8>>,
    /// Whether the answer came from another URL than the one asked, after one or more redirects.
    redirected: bool,
}

/// Sends one request. The limit covers the wait for the answer and the reading of its body, but only a failure
/// before the answer starts is a fault: a body that fails or times out later is reported as an unreadable body.
async fn exchange<H: HttpExecute>(http: &H, request: reqwest::Request, limit: Duration) -> Result<Reply, Fault> {
    let asked = request.url().clone();
    let deadline = tokio::time::Instant::now() + limit;
    let mut response = match tokio::time::timeout_at(deadline, http.execute(request)).await {
        Err(_) => return Err(Fault::TimedOut),
        Ok(Err(error)) => return Err(Fault::Unreachable(Box::new(error))),
        Ok(Ok(response)) => response,
    };
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let redirected = *response.url() != asked;
    let read = async {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            body.extend_from_slice(&chunk);
        }
        Ok::<Vec<u8>, reqwest::Error>(body)
    };
    let body = tokio::time::timeout_at(deadline, read).await.ok().and_then(Result::ok);
    Ok(Reply {
        status,
        headers,
        body,
        redirected,
    })
}

/// The body as the service client reads it: JSON when it parses, otherwise the text itself.
fn decode(bytes: &[u8]) -> Value {
    let text = decode_text(bytes);
    if text.is_empty() {
        return Value::String(String::new());
    }
    parse_json(&text).unwrap_or(Value::String(text))
}

fn worker_epoch(data: &Value) -> Result<i64, RegisterFailure> {
    let number = match data.get("worker_epoch") {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => Some(js_number(text)),
        _ => None,
    };
    number
        .and_then(safe_integer)
        .ok_or_else(|| RegisterFailure::InvalidEpoch {
            detail: plain_detail(&js_stringify(data)),
        })
}

/// The reason a `409` gives for refusing the worker: the header first, then `error.reason`, `reason` and `error.type`
/// of the body.
fn conflict_reason(headers: &HeaderMap, body: Option<&[u8]>) -> Option<MismatchReason> {
    if let Some(reason) = header_text(headers, "x-ccr-conflict-reason")
        .as_deref()
        .and_then(MismatchReason::from_conflict)
    {
        return Some(reason);
    }
    let data = parse_json(&decode_text(body?)).ok()?;
    let error = data.get("error");
    let named = error
        .and_then(|error| error.get("reason"))
        .filter(|reason| !reason.is_null())
        .or_else(|| data.get("reason"));
    named
        .and_then(Value::as_str)
        .and_then(MismatchReason::from_conflict)
        .or_else(|| {
            error
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str)
                .and_then(MismatchReason::from_conflict)
        })
}

fn valid_ticket(value: &str) -> bool {
    (1..=4096).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The `x-ccr-ticket-*` headers the service handed out, which the worker echoes on its next requests. They belong
/// to the `Authorization` value they were handed out for: another value drops them.
#[derive(Default)]
struct Tickets {
    authorization: Option<Vec<u8>>,
    held: Vec<(&'static str, String)>,
}

impl Tickets {
    /// The headers to echo for a request that carries `authorization`.
    fn headers_for(&mut self, authorization: &[u8]) -> Vec<(&'static str, String)> {
        if self.authorization.as_deref() != Some(authorization) {
            self.held.clear();
            self.authorization = Some(authorization.to_owned());
        }
        self.held.clone()
    }

    fn session_ticket_for(&self, authorization: &[u8]) -> Option<&str> {
        if self.authorization.as_deref() != Some(authorization) {
            return None;
        }
        self.held
            .iter()
            .find(|(name, _)| *name == SESSION_TICKET_HEADER)
            .map(|(_, value)| value.as_str())
    }

    fn note(&mut self, authorization: &[u8], redirected: bool, status: u16, headers: &HeaderMap) {
        if self.authorization.as_deref() != Some(authorization) {
            return;
        }
        if redirected || ((400..500).contains(&status) && status != 429) {
            self.held.clear();
            return;
        }
        for name in TICKET_HEADERS {
            let Some(value) = header_text(headers, name) else {
                continue;
            };
            if value.is_empty() {
                self.held.retain(|(held, _)| *held != name);
            } else if valid_ticket(&value) {
                match self.held.iter_mut().find(|(held, _)| *held == name) {
                    Some(entry) => entry.1 = value,
                    None => self.held.push((name, value)),
                }
            }
        }
    }
}

/// Talks to the worker endpoints of one remote session the way the upstream bridge entrypoint does.
pub struct WorkerClient<H, P = TokioPacing> {
    http: Arc<H>,
    pacing: Arc<P>,
    session_url: String,
    session_id: String,
    token: SensitiveString,
    platform: &'static str,
    epoch: i64,
    state: SessionState,
    heartbeat_interval_ms: f64,
    consecutive_auth_failures: u32,
    consecutive_not_found: u32,
    tickets: Tickets,
    session_access_token: Option<SensitiveString>,
    non_origin_403: NonOrigin403,
    queue: PatchQueue,
    events: EventQueue,
    dropped_durable_event_batches: u32,
    record_at_start: Option<Map<String, Value>>,
    reported_keys: Vec<String>,
    reply_degraded: bool,
    init_state_get_ordering_bound: Duration,
    closed: bool,
    dropped_state_patches: u32,
    dropped_owed_metadata_patches: u32,
}

impl<H, P> WorkerClient<H, P> {
    pub fn new(http: H, pacing: P, config: WorkerConfig) -> Result<Self, InvalidSessionId> {
        let session_url = session_url(&config.api_base_url, &config.session_id)?;
        let interval = config.heartbeat_interval_ms.unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_MS);
        Ok(Self {
            http: Arc::new(http),
            pacing: Arc::new(pacing),
            session_url,
            session_id: config.session_id,
            token: config.token,
            platform: client_platform(config.entrypoint.as_deref().unwrap_or("")),
            epoch: 0,
            state: SessionState::Idle,
            heartbeat_interval_ms: interval as f64,
            consecutive_auth_failures: 0,
            consecutive_not_found: 0,
            tickets: Tickets::default(),
            session_access_token: config.session_access_token,
            non_origin_403: config.non_origin_403,
            queue: PatchQueue::new(),
            events: EventQueue::default(),
            dropped_durable_event_batches: 0,
            record_at_start: None,
            reported_keys: Vec::new(),
            reply_degraded: false,
            init_state_get_ordering_bound: INIT_STATE_GET_ORDERING_BOUND,
            closed: false,
            dropped_state_patches: 0,
            dropped_owed_metadata_patches: 0,
        })
    }

    /// Replaces the token the requests carry. The tickets held for the old `Authorization` value are dropped: they
    /// belong to it.
    pub fn set_token(&mut self, token: SensitiveString) {
        self.token = token;
    }

    /// Replaces the `CLAUDE_CODE_SESSION_ACCESS_TOKEN` value a `401` or `403` is checked against.
    pub fn set_session_access_token(&mut self, token: Option<SensitiveString>) {
        self.session_access_token = token;
    }

    /// `sessionTicketFor`: the session ticket held for the token the requests carry now, if any.
    pub fn session_ticket(&self) -> Option<&str> {
        let authorization = format!("Bearer {}", self.token.expose());
        let authorization = header_value(&authorization, true)?;
        self.tickets.session_ticket_for(authorization.as_bytes())
    }

    pub fn session_url(&self) -> &str {
        &self.session_url
    }

    pub fn epoch(&self) -> i64 {
        self.epoch
    }

    /// The state the worker last reported, or `Idle` before it reported any.
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// The current beat interval: the one the worker started with, or the last one the service advised.
    pub fn heartbeat_interval_ms(&self) -> f64 {
        self.heartbeat_interval_ms
    }

    fn authorization(&self, for_fetch: bool) -> Option<HeaderValue> {
        header_value(&format!("Bearer {}", self.token.expose()), for_fetch)
    }
}

impl<H: HttpExecute, P: WorkerPacing> WorkerClient<H, P> {
    /// `registerWorker`: asks the service for the epoch this worker will run under.
    pub async fn register_worker(&self) -> Result<i64, RegisterFailure> {
        let named = |message: String, code: &str| RegisterFailure::Network {
            message,
            code: Some(code.to_owned()),
        };
        let url = reqwest::Url::parse(&format!("{}/worker/register", self.session_url))
            .map_err(|_| named("Invalid URL".to_owned(), "ERR_INVALID_URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(named(
                format!("Unsupported protocol {}:", url.scheme()),
                "ERR_BAD_REQUEST",
            ));
        }
        let authorization = self.authorization(false).ok_or_else(|| {
            named(
                "Invalid character in header content [\"Authorization\"]".to_owned(),
                "ERR_INVALID_CHAR",
            )
        })?;
        let target = url.clone();
        let mut request = reqwest::Request::new(Method::POST, url);
        let headers = request.headers_mut();
        headers.insert("accept", HeaderValue::from_static("application/json, text/plain, */*"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("authorization", authorization);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        *request.body_mut() = Some("{}".as_bytes().to_vec().into());
        let reply = exchange(&*self.http, request, REGISTER_TIMEOUT)
            .await
            .map_err(|fault| match fault {
                Fault::TimedOut => RegisterFailure::TimedOut,
                Fault::Unreachable(error) => network_failure(&target, &error),
            })?;
        let Some(body) = &reply.body else {
            return Err(named("stream has been aborted".to_owned(), "ERR_BAD_RESPONSE"));
        };
        if !(200..300).contains(&reply.status) {
            return Err(RegisterFailure::Status(reply.status));
        }
        worker_epoch(&decode(body))
    }

    /// The first `PUT /worker`: registers the worker as idle and retries what is worth retrying. The stored record is
    /// read at the same time (`GET /worker`, bridge.mjs offset 1049109): the registration starts once the read is
    /// answered or `initStateGetOrderingBoundMs` has passed, and a read still pending when the registration is
    /// accepted is abandoned, which leaves the stored record unknown, as `stateReadEndsAtRegistration` does for an
    /// attached session (offset 1196900, where `onConnectNeedsRecord` is never set).
    pub async fn initialize(&mut self, epoch: i64) -> Result<(), InitError> {
        self.epoch = epoch;
        let mut read = Box::pin(read_worker_record(
            self.http.clone(),
            self.pacing.clone(),
            self.authorization(true),
            reqwest::Url::parse(&format!("{}/worker", self.session_url)).ok(),
            self.platform,
        ));
        let mut answered = None;
        tokio::select! {
            result = &mut read => answered = Some(result),
            () = tokio::time::sleep(self.init_state_get_ordering_bound) => {}
        }
        if let Some(Err(reason)) = answered {
            return Err(InitError::Mismatch(reason));
        }
        let body = json!({
            "worker_status": "idle",
            "worker_epoch": epoch,
            "external_metadata": {"pending_action": null, "pending_actions": null, "task_summary": null},
        });
        let registered = {
            let registration = self.register_idle(&body);
            tokio::pin!(registration);
            loop {
                tokio::select! {
                    result = &mut registration => break result,
                    result = &mut read, if answered.is_none() => answered = Some(result),
                }
            }
        };
        registered?;
        match answered {
            Some(Err(reason)) => return Err(InitError::Mismatch(reason)),
            Some(Ok(record)) => self.record_at_start = record,
            None => {}
        }
        Ok(())
    }

    async fn register_idle(&mut self, body: &Value) -> Result<(), InitError> {
        let mut attempt = 0;
        let failure = loop {
            attempt += 1;
            let outcome = self
                .request(Method::PUT, "/worker", body, REQUEST_TIMEOUT, false)
                .await
                .map_err(InitError::Mismatch)?;
            let RequestOutcome::Failed(failure) = outcome else {
                self.state = SessionState::Idle;
                return Ok(());
            };
            if matches!(failure.status, Some(400 | 413 | 422)) || attempt >= INIT_ATTEMPTS {
                break failure;
            }
            let backoff = (500_u64 << (attempt - 1)).min(INIT_BACKOFF_CEILING_MS);
            let delay = Duration::from_millis(backoff) + Duration::from_secs_f64(self.pacing.jitter() * 0.5);
            self.pacing.pause(delay).await;
        };
        Err(InitError::Failed(InitFailure {
            reason: "worker_register_failed",
            http_status: failure.status,
            no_answer: failure.fetch_failed,
        }))
    }

    /// Reports the worker state. Nothing is sent when the state is the one already reported and no details ride
    /// along; the outcome is `None` then.
    pub async fn report_state(
        &mut self,
        state: SessionState,
        details: Option<&RequiresActionDetails>,
    ) -> Result<Option<RequestOutcome>, MismatchReason> {
        if state == self.state && details.is_none() {
            return Ok(None);
        }
        self.state = state;
        let body = json!({
            "worker_epoch": self.epoch,
            "worker_status": state.as_str(),
            "requires_action_details": details.map_or(Value::Null, RequiresActionDetails::to_json),
            "internal_metadata": {"reply_degraded": self.reply_degraded},
        });
        self.request(Method::PUT, "/worker", &body, REQUEST_TIMEOUT, false)
            .await
            .map(Some)
    }

    /// Reports `external_metadata` for the worker. The keys the stored record already holds are left out the first
    /// time they are reported (`withoutRepeatsOfRecord`); the outcome is `None` when nothing is left to send.
    pub async fn report_metadata(
        &mut self,
        metadata: &Map<String, Value>,
    ) -> Result<Option<RequestOutcome>, MismatchReason> {
        let Some(metadata) = self.without_repeats_of_record(metadata) else {
            return Ok(None);
        };
        let body = json!({
            "worker_epoch": self.epoch,
            "external_metadata": external_metadata_patch(&metadata),
        });
        self.request(Method::PUT, "/worker", &body, REQUEST_TIMEOUT, false)
            .await
            .map(Some)
    }

    /// Queues a state report the way `reportState` does: nothing is queued when the state is the one already
    /// reported and no details ride along. Returns whether a patch was queued. [`WorkerClient::flush`] sends it.
    pub fn submit_state(&mut self, state: SessionState, details: Option<&RequiresActionDetails>) -> bool {
        if state == self.state && details.is_none() {
            return false;
        }
        self.state = state;
        let mut patch = Patch::new();
        patch.insert("worker_status".to_owned(), json!(state.as_str()));
        patch.insert(
            "requires_action_details".to_owned(),
            details.map_or(Value::Null, RequiresActionDetails::to_json),
        );
        patch.insert(
            "internal_metadata".to_owned(),
            json!({"reply_degraded": self.reply_degraded}),
        );
        self.queue.enqueue(patch);
        true
    }

    /// Queues `external_metadata` the way `reportMetadata` does, leaving out the keys the stored record already
    /// holds the first time they are reported. Returns whether a patch was queued. [`WorkerClient::flush`] sends it.
    pub fn submit_metadata(&mut self, metadata: &Map<String, Value>) -> bool {
        let Some(metadata) = self.without_repeats_of_record(metadata) else {
            return false;
        };
        let mut patch = Patch::new();
        patch.insert(
            "external_metadata".to_owned(),
            Value::Object(external_metadata_patch(&metadata)),
        );
        self.queue.enqueue(patch);
        true
    }

    /// `withoutRepeatsOfRecord`: the metadata without the keys that were first reported now and equal what the
    /// record held at the start. `None` when that leaves nothing of a non-empty report.
    fn without_repeats_of_record(&mut self, metadata: &Map<String, Value>) -> Option<Map<String, Value>> {
        let mut repeated: Vec<&str> = Vec::new();
        for key in metadata.keys() {
            if !RECORDED_KEYS.contains(&key.as_str()) || self.reported_keys.iter().any(|seen| seen == key) {
                continue;
            }
            self.reported_keys.push(key.clone());
            let stored = self.record_at_start.as_ref().and_then(|record| record.get(key));
            if self.record_at_start.is_some() && js_equal(metadata.get(key), stored) {
                repeated.push(key);
            }
        }
        if repeated.is_empty() {
            return Some(metadata.clone());
        }
        let left: Map<String, Value> = metadata
            .iter()
            .filter(|(key, _)| !repeated.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        (!left.is_empty()).then_some(left)
    }

    /// Stops the queue from taking new patches; see [`PatchQueue::close_to_new_patches`].
    pub fn close_queue_to_new_patches(&mut self) {
        self.queue.close_to_new_patches();
    }

    /// Drops what the queue still holds and stops it for good.
    pub fn close_queue(&mut self) {
        self.closed = true;
        self.queue.close();
    }

    /// How many patches the client gave up on (`droppedWorkerStatePatchCount`, bridge.mjs offset 1128273).
    pub fn dropped_state_patches(&self) -> u32 {
        self.dropped_state_patches
    }

    /// How many of those still owed metadata (`droppedOwedMetadataPatchCount`).
    pub fn dropped_owed_metadata_patches(&self) -> u32 {
        self.dropped_owed_metadata_patches
    }

    /// Rust only. Sets the `external_metadata` the stored record held when the session started (`recordAtStart`);
    /// `initialize` sets it from the read it makes.
    pub fn set_record_at_start(&mut self, record: Option<Map<String, Value>>) {
        self.record_at_start = record;
    }

    /// Rust only. Sets `initStateGetOrderingBoundMs` (offset 1049109, `dHe` = 10 seconds), which no public option of
    /// the bridge reaches; it lets a test keep the ordering exact without waiting out the real bound.
    /// Rust only. Sets the `reply_degraded` value every status report carries. The bridge evaluates `e3()` (offset
    /// 856241) to false for an attached session, so the value is false unless a caller sets it.
    pub fn set_init_state_get_ordering_bound(&mut self, bound: Duration) {
        self.init_state_get_ordering_bound = bound;
    }

    pub fn set_reply_degraded(&mut self, degraded: bool) {
        self.reply_degraded = degraded;
    }

    /// The queue behind [`WorkerClient::submit_state`] and [`WorkerClient::submit_metadata`].
    pub fn queue(&self) -> &PatchQueue {
        &self.queue
    }

    /// Sends what is queued, retrying with the upstream backoff until the service takes every patch. A `400`,
    /// `413` or `422` drops the patch and puts back what must still reach the service. Returns whether everything
    /// went out and nothing was dropped; a mismatch ends the flush.
    pub async fn flush(&mut self) -> Result<bool, MismatchReason> {
        let drained = self.pump(None).await?;
        Ok(drained && !self.queue.has_dropped_something() && !self.queue.is_closed())
    }

    /// [`WorkerClient::flush`] that stops once `attempts` requests were made and another request or a retry wait
    /// is next, so patches queued meanwhile are seen by the wait that follows. Returns whether the queue drained.
    pub async fn pump(&mut self, attempts: Option<u32>) -> Result<bool, MismatchReason> {
        let mut made = 0_u32;
        loop {
            let capped = attempts.is_some_and(|cap| made >= cap);
            match self.queue.poll(self.pacing.jitter()) {
                Next::Idle => return Ok(true),
                Next::Pause(_) | Next::Send(_) if capped => return Ok(false),
                Next::Pause(delay) => self.pacing.pause(delay).await,
                Next::Send(patch) => {
                    made += 1;
                    let mut body = Patch::new();
                    body.insert("worker_epoch".to_owned(), json!(self.epoch));
                    for (key, value) in &patch {
                        body.insert(key.clone(), value.clone());
                    }
                    let outcome = self
                        .request(Method::PUT, "/worker", &Value::Object(body), REQUEST_TIMEOUT, false)
                        .await?;
                    match outcome {
                        RequestOutcome::Accepted(_) => self.queue.sent(true),
                        RequestOutcome::Failed(failure) if self.closed && failure.status == Some(409) => {
                            self.dropped_state_patches += 1;
                            if without_status(&patch).is_some() {
                                self.dropped_owed_metadata_patches += 1;
                            }
                            self.queue.sent(true);
                        }
                        RequestOutcome::Failed(failure) if matches!(failure.status, Some(400 | 413 | 422)) => {
                            self.dropped_state_patches += 1;
                            if without_status(&patch).is_some() {
                                self.dropped_owed_metadata_patches += 1;
                            }
                            let back = requeue_after_refusal(&patch);
                            if let Some(metadata) = back.metadata {
                                self.queue.requeue_metadata(metadata);
                            }
                            if let Some(status) = back.status {
                                self.queue.requeue_status(status);
                            }
                            self.queue.sent(true);
                        }
                        RequestOutcome::Failed(_) => self.queue.sent(false),
                    }
                }
            }
        }
    }

    /// Queues client events for `POST /worker/events` (`writeEvents`). An event without a string `uuid` gets one.
    /// When the queue is full the call sends until there is room, as the upstream writer waits for it.
    pub async fn submit_events(&mut self, events: Vec<ClientEvent>) -> Result<(), MismatchReason> {
        let events: Vec<ClientEvent> = events.into_iter().map(with_uuid).collect();
        for chunk in events.chunks(self.events_capacity()) {
            while !self.events.has_room_for(chunk.len()) && !self.events.is_closed() {
                self.pump_events(Some(1)).await?;
            }
            self.events.enqueue(chunk.to_vec());
        }
        Ok(())
    }

    fn events_capacity(&self) -> usize {
        self.events.capacity()
    }

    /// Sends the queued events, retrying with the upstream backoff until the service takes every batch. Returns
    /// whether the queue drained; a mismatch ends the flush.
    pub async fn flush_events(&mut self) -> Result<bool, MismatchReason> {
        self.pump_events(None).await
    }

    /// How many batches of durable events the service refused for good and the client dropped.
    pub fn dropped_durable_event_batches(&self) -> u32 {
        self.dropped_durable_event_batches
    }

    /// Closes the event queue (`close`); with `retain` the events not yet taken stay available to the caller.
    pub fn close_events(&mut self, retain: bool) -> usize {
        self.events.close(retain)
    }

    /// The events kept by [`WorkerClient::close_events`].
    pub fn take_undelivered_events(&mut self) -> Vec<ClientEvent> {
        self.events.take_undelivered()
    }

    /// The event queue.
    pub fn event_queue(&self) -> &EventQueue {
        &self.events
    }

    /// [`WorkerClient::flush_events`] that stops once `attempts` requests were made and another request or a retry
    /// wait is next.
    pub async fn pump_events(&mut self, attempts: Option<u32>) -> Result<bool, MismatchReason> {
        let mut made = 0_u32;
        loop {
            if attempts.is_some_and(|cap| made >= cap) {
                return Ok(self.events.is_idle());
            }
            match self.events.poll() {
                EventNext::Idle => return Ok(true),
                EventNext::Pause(delay) => self.pacing.pause(delay).await,
                EventNext::Send(batch) => {
                    made += 1;
                    let timeout = upload_timeout(batch.bytes, batch.cuttable, batch.retry_count);
                    let body = upload_body(self.epoch, &batch.events);
                    let outcome = self
                        .request_with(
                            Method::POST,
                            "/worker/events",
                            &body,
                            timeout,
                            false,
                            Some(batch.attempt.as_str()),
                        )
                        .await?;
                    self.take_event_answer(outcome).await?;
                }
            }
        }
    }

    /// What the upload does with the answer to a batch (the end of the `send` of the client event uploader, bridge.mjs
    /// offset 1036000 to 1040000).
    async fn take_event_answer(&mut self, outcome: RequestOutcome) -> Result<(), MismatchReason> {
        let RequestOutcome::Failed(failure) = outcome else {
            self.events.done();
            return Ok(());
        };
        let status = failure.status;
        if self.closed && status == Some(409) && self.events.in_flight_events().iter().all(|event| event.ephemeral) {
            self.events.done();
            return Ok(());
        }
        if status.is_some_and(refused_for_good) {
            let dropped = self.events.keep_only(|event| !event.ephemeral);
            let durable = self.events.in_flight_events();
            if dropped > 0 {
                if durable.is_empty() {
                    self.events.done();
                    return Ok(());
                }
                let body = upload_body(self.epoch, &durable);
                let bytes = durable.iter().map(ClientEvent::json_bytes).sum();
                let timeout = upload_timeout(bytes, durable.len() > 1, 0);
                let retry = self
                    .request_with(Method::POST, "/worker/events", &body, timeout, false, Some("resend"))
                    .await?;
                match retry {
                    RequestOutcome::Accepted(_) => {
                        self.events.done();
                        return Ok(());
                    }
                    RequestOutcome::Failed(again) if !again.status.is_some_and(refused_for_good) => {
                        let jitter = self.pacing.jitter();
                        self.events.failed(jitter, again.retry_after_ms, again.fetch_failed);
                        return Ok(());
                    }
                    RequestOutcome::Failed(_) => {}
                }
            }
            self.dropped_durable_event_batches += 1;
            self.events.done();
            return Ok(());
        }
        let jitter = self.pacing.jitter();
        self.events.failed(jitter, failure.retry_after_ms, failure.fetch_failed);
        Ok(())
    }

    /// One heartbeat. An accepted answer that advises a beat interval changes the interval the worker keeps.
    pub async fn heartbeat(&mut self) -> Result<RequestOutcome, MismatchReason> {
        let body = json!({"session_id": self.session_id, "worker_epoch": self.epoch});
        let timeout = heartbeat_timeout(self.heartbeat_interval_ms);
        let outcome = self
            .request(Method::POST, "/worker/heartbeat", &body, timeout, true)
            .await?;
        if let RequestOutcome::Accepted(Some(answer)) = &outcome
            && let Some(seconds) = answer.get("heartbeat_interval_seconds").and_then(Value::as_f64)
            && seconds > 0.0
        {
            let floor = HEARTBEAT_INTERVAL_FLOOR_MS as f64;
            let ceiling = HEARTBEAT_INTERVAL_CEILING_MS as f64;
            self.heartbeat_interval_ms = (seconds * 1000.0).clamp(floor, ceiling);
        }
        Ok(outcome)
    }

    async fn request(
        &mut self,
        method: Method,
        path: &str,
        body: &Value,
        timeout: Duration,
        parse_body: bool,
    ) -> Result<RequestOutcome, MismatchReason> {
        self.request_with(method, path, body, timeout, parse_body, None).await
    }

    /// [`WorkerClient::request`] that also tells the service whether it may have seen the rows before
    /// (`x-ccr-upload-attempt`).
    async fn request_with(
        &mut self,
        method: Method,
        path: &str,
        body: &Value,
        timeout: Duration,
        parse_body: bool,
        upload_attempt: Option<&'static str>,
    ) -> Result<RequestOutcome, MismatchReason> {
        let unanswered = |reason: &str| {
            Ok(RequestOutcome::Failed(RequestFailure {
                status: None,
                reason: reason.to_owned(),
                retry_after_ms: None,
                fetch_failed: true,
                non_origin_refusal: false,
            }))
        };
        let (Some(authorization), Ok(url)) = (
            self.authorization(true),
            reqwest::Url::parse(&format!("{}{path}", self.session_url)),
        ) else {
            return unanswered("fetch_failed:TypeError");
        };
        let key = authorization.as_bytes().to_vec();
        let held = self.tickets.headers_for(&key);
        let mut request = reqwest::Request::new(method, url);
        let headers = request.headers_mut();
        headers.insert("authorization", authorization);
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("anthropic-client-platform", HeaderValue::from_static(self.platform));
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        for (name, value) in &held {
            if let Ok(value) = HeaderValue::from_str(value) {
                headers.insert(*name, value);
            }
        }
        if let Some(attempt) = upload_attempt {
            headers.insert("x-ccr-upload-attempt", HeaderValue::from_static(attempt));
        }
        headers.insert("accept", HeaderValue::from_static("*/*"));
        *request.body_mut() = Some(js_stringify(body).into_bytes().into());
        let reply = match exchange(&*self.http, request, timeout).await {
            Ok(reply) => reply,
            Err(Fault::TimedOut) => return unanswered("fetch_failed:TimeoutError"),
            Err(Fault::Unreachable(_)) => return unanswered("fetch_failed:TypeError"),
        };
        let status = reply.status;
        self.tickets.note(&key, reply.redirected, status, &reply.headers);
        if (200..300).contains(&status) {
            self.consecutive_auth_failures = 0;
            self.consecutive_not_found = 0;
            let data = parse_body
                .then(|| {
                    reply
                        .body
                        .as_deref()
                        .and_then(|bytes| parse_json(&decode_text(bytes)).ok())
                })
                .flatten();
            return Ok(RequestOutcome::Accepted(data));
        }
        if self.closed {
            return Ok(RequestOutcome::Failed(RequestFailure {
                status: Some(status),
                reason: format!("http_{status}"),
                retry_after_ms: None,
                fetch_failed: false,
                non_origin_refusal: false,
            }));
        }
        if status == 409 {
            return Err(conflict_reason(&reply.headers, reply.body.as_deref()).unwrap_or(MismatchReason::EpochConflict));
        }
        if status == 404 {
            self.consecutive_not_found += 1;
            if self.consecutive_not_found >= NOT_FOUND_BEFORE_EXIT {
                return Err(MismatchReason::SessionNotFound);
            }
        }
        let non_origin_refusal = status == 403
            && self.non_origin_403 == NonOrigin403::Transient
            && refusal_source(&reply.headers).is_non_origin();
        if matches!(status, 401 | 403) {
            let expiry = self
                .session_access_token
                .as_ref()
                .filter(|token| !token.expose().is_empty())
                .and_then(|token| token_expiry(token.expose()));
            if expiry.is_some_and(|seconds| seconds * 1000.0 < now_ms()) {
                return Err(MismatchReason::TokenExpired);
            }
            if !non_origin_refusal {
                self.consecutive_auth_failures += 1;
                if self.consecutive_auth_failures >= AUTH_FAILURES_BEFORE_EXIT {
                    return Err(MismatchReason::AuthExhausted);
                }
            }
        }
        let retry_after_ms = (status == 429)
            .then(|| header_text(&reply.headers, "retry-after"))
            .flatten()
            .and_then(|text| retry_after_ms(&text));
        Ok(RequestOutcome::Failed(RequestFailure {
            status: Some(status),
            reason: format!("http_{status}"),
            retry_after_ms,
            fetch_failed: false,
            non_origin_refusal,
        }))
    }
}

/// `getWorkerState` as `initialize` runs it: `GET /worker` with up to ten attempts, bridge.mjs offset 1049109. The
/// answer is the `external_metadata` the stored record holds (empty when it holds none), or `None` when the read
/// failed for good.
async fn read_worker_record<H: HttpExecute, P: WorkerPacing>(
    http: Arc<H>,
    pacing: Arc<P>,
    authorization: Option<HeaderValue>,
    url: Option<reqwest::Url>,
    platform: &'static str,
) -> Result<Option<Map<String, Value>>, MismatchReason> {
    let (Some(authorization), Some(url)) = (authorization, url) else {
        return Ok(None);
    };
    for attempt in 1..=GET_ATTEMPTS {
        let mut request = reqwest::Request::new(Method::GET, url.clone());
        let headers = request.headers_mut();
        headers.insert("authorization", authorization.clone());
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("anthropic-client-platform", HeaderValue::from_static(platform));
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        if let Ok(reply) = exchange(&*http, request, GET_TIMEOUT).await {
            let status = reply.status;
            if (200..300).contains(&status) {
                let data = reply
                    .body
                    .as_deref()
                    .and_then(|bytes| parse_json(&decode_text(bytes)).ok());
                if let Some(data) = data {
                    let record = data
                        .get("worker")
                        .and_then(|worker| worker.get("external_metadata"))
                        .and_then(Value::as_object)
                        .cloned();
                    return Ok(Some(record.unwrap_or_default()));
                }
            } else if status == 409 {
                return Err(
                    conflict_reason(&reply.headers, reply.body.as_deref()).unwrap_or(MismatchReason::EpochConflict)
                );
            } else if matches!(status, 400 | 413 | 422) {
                return Ok(None);
            }
        }
        if attempt < GET_ATTEMPTS {
            let backoff = (500_u64 << (attempt - 1)).min(INIT_BACKOFF_CEILING_MS);
            pacing
                .pause(Duration::from_millis(backoff) + Duration::from_secs_f64(pacing.jitter() * 0.5))
                .await;
        }
    }
    Ok(None)
}

/// An event that has no string `uuid` gets a random one (`toClientEvent`).
fn with_uuid(mut event: ClientEvent) -> ClientEvent {
    if !event.payload.get("uuid").is_some_and(Value::is_string) {
        event
            .payload
            .insert("uuid".to_owned(), Value::String(uuid::Uuid::new_v4().to_string()));
    }
    event
}

fn decode_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned()
}

/// `Jb(a ?? void 0, b ?? void 0)`: bridge.mjs offset 837828 compares deeply, with a missing top-level value and
/// null alike.
fn js_equal(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left.filter(|v| !v.is_null()), right.filter(|v| !v.is_null())) {
        (None, None) => true,
        (Some(a), Some(b)) => deep_equal(a, b),
        _ => false,
    }
}

fn deep_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| deep_equal(x, y)),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(key, x)| b.get(key).is_some_and(|y| deep_equal(x, y)))
        }
        (a, b) => a == b,
    }
}

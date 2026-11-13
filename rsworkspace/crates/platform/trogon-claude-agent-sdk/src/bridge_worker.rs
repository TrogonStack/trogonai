//! The worker half of the bridge: registering a worker for a remote session and keeping the service told about it
//! (`PUT /worker` state and metadata, `POST /worker/heartbeat`), with the request path rules the upstream bridge
//! entrypoint applies to every answer.

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Method;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Map, Value, json};

use crate::SensitiveString;
use crate::adapters::HttpExecute;
use crate::bridge_client::{USER_AGENT, client_platform, header_text};
use crate::bridge_js::{js_number, js_parse_int, plain_detail, safe_integer};
use crate::bridge_types::SessionState;
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
const INIT_BACKOFF_CEILING_MS: u64 = 30_000;
const TICKET_HEADERS: [&str; 3] = ["x-ccr-ticket-rev", "x-ccr-ticket-session", "x-ccr-ticket-seq"];

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
}

enum Fault {
    TimedOut,
    Unreachable,
}

struct Reply {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

async fn exchange<H: HttpExecute>(http: &H, request: reqwest::Request, limit: Duration) -> Result<Reply, Fault> {
    let work = async {
        let mut response = http.execute(request).await.map_err(|_| Fault::Unreachable)?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Fault::Unreachable)? {
            body.extend_from_slice(&chunk);
        }
        Ok::<Reply, Fault>(Reply { status, headers, body })
    };
    tokio::time::timeout(limit, work).await.map_err(|_| Fault::TimedOut)?
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
fn conflict_reason(headers: &HeaderMap, body: &[u8]) -> Option<MismatchReason> {
    if let Some(reason) = header_text(headers, "x-ccr-conflict-reason")
        .as_deref()
        .and_then(MismatchReason::from_conflict)
    {
        return Some(reason);
    }
    let data = decode(body);
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

/// The `x-ccr-ticket-*` headers the service handed out, which the worker echoes on its next requests.
#[derive(Default)]
struct Tickets {
    held: Vec<(&'static str, String)>,
}

impl Tickets {
    fn note(&mut self, status: u16, headers: &HeaderMap) {
        if (400..500).contains(&status) && status != 429 {
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
    http: H,
    pacing: P,
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
}

impl<H, P> WorkerClient<H, P> {
    pub fn new(http: H, pacing: P, config: WorkerConfig) -> Result<Self, InvalidSessionId> {
        let session_url = session_url(&config.api_base_url, &config.session_id)?;
        let interval = config.heartbeat_interval_ms.unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_MS);
        Ok(Self {
            http,
            pacing,
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
        })
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

    fn authorization(&self) -> Option<HeaderValue> {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", self.token.expose())).ok()?;
        value.set_sensitive(true);
        Some(value)
    }
}

impl<H: HttpExecute, P: WorkerPacing> WorkerClient<H, P> {
    /// `registerWorker`: asks the service for the epoch this worker will run under.
    pub async fn register_worker(&self) -> Result<i64, RegisterFailure> {
        let url = reqwest::Url::parse(&format!("{}/worker/register", self.session_url))
            .map_err(|_| RegisterFailure::Unreachable)?;
        let authorization = self.authorization().ok_or(RegisterFailure::Unreachable)?;
        let mut request = reqwest::Request::new(Method::POST, url);
        let headers = request.headers_mut();
        headers.insert("accept", HeaderValue::from_static("application/json, text/plain, */*"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("authorization", authorization);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        *request.body_mut() = Some("{}".as_bytes().to_vec().into());
        let reply = exchange(&self.http, request, REGISTER_TIMEOUT)
            .await
            .map_err(|fault| match fault {
                Fault::TimedOut => RegisterFailure::TimedOut,
                Fault::Unreachable => RegisterFailure::Unreachable,
            })?;
        if !(200..300).contains(&reply.status) {
            return Err(RegisterFailure::Status(reply.status));
        }
        worker_epoch(&decode(&reply.body))
    }

    /// The first `PUT /worker`: registers the worker as idle and retries what is worth retrying.
    pub async fn initialize(&mut self, epoch: i64) -> Result<(), InitError> {
        self.epoch = epoch;
        let body = json!({
            "worker_status": "idle",
            "worker_epoch": epoch,
            "external_metadata": {"pending_action": null, "pending_actions": null, "task_summary": null},
        });
        let mut attempt = 0;
        let failure = loop {
            attempt += 1;
            let outcome = self
                .request(Method::PUT, "/worker", &body, REQUEST_TIMEOUT, false)
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
            "internal_metadata": {"reply_degraded": false},
        });
        self.request(Method::PUT, "/worker", &body, REQUEST_TIMEOUT, false)
            .await
            .map(Some)
    }

    /// Reports `external_metadata` for the worker.
    pub async fn report_metadata(&mut self, metadata: &Map<String, Value>) -> Result<RequestOutcome, MismatchReason> {
        let body = json!({
            "worker_epoch": self.epoch,
            "external_metadata": external_metadata_patch(metadata),
        });
        self.request(Method::PUT, "/worker", &body, REQUEST_TIMEOUT, false)
            .await
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
        let unanswered = |reason: &str| {
            Ok(RequestOutcome::Failed(RequestFailure {
                status: None,
                reason: reason.to_owned(),
                retry_after_ms: None,
                fetch_failed: true,
            }))
        };
        let (Some(authorization), Ok(url)) = (
            self.authorization(),
            reqwest::Url::parse(&format!("{}{path}", self.session_url)),
        ) else {
            return unanswered("fetch_failed:TypeError");
        };
        let mut request = reqwest::Request::new(method, url);
        let headers = request.headers_mut();
        headers.insert("authorization", authorization);
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("anthropic-client-platform", HeaderValue::from_static(self.platform));
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        for (name, value) in &self.tickets.held {
            if let Ok(value) = HeaderValue::from_str(value) {
                headers.insert(*name, value);
            }
        }
        headers.insert("accept", HeaderValue::from_static("*/*"));
        *request.body_mut() = Some(js_stringify(body).into_bytes().into());
        let reply = match exchange(&self.http, request, timeout).await {
            Ok(reply) => reply,
            Err(Fault::TimedOut) => return unanswered("fetch_failed:TimeoutError"),
            Err(Fault::Unreachable) => return unanswered("fetch_failed:TypeError"),
        };
        let status = reply.status;
        self.tickets.note(status, &reply.headers);
        if (200..300).contains(&status) {
            self.consecutive_auth_failures = 0;
            self.consecutive_not_found = 0;
            let data = parse_body.then(|| parse_json(&decode_text(&reply.body)).ok()).flatten();
            return Ok(RequestOutcome::Accepted(data));
        }
        if status == 409 {
            return Err(conflict_reason(&reply.headers, &reply.body).unwrap_or(MismatchReason::EpochConflict));
        }
        if status == 404 {
            self.consecutive_not_found += 1;
            if self.consecutive_not_found >= NOT_FOUND_BEFORE_EXIT {
                return Err(MismatchReason::SessionNotFound);
            }
        }
        if matches!(status, 401 | 403) {
            self.consecutive_auth_failures += 1;
            if self.consecutive_auth_failures >= AUTH_FAILURES_BEFORE_EXIT {
                return Err(MismatchReason::AuthExhausted);
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
        }))
    }
}

fn decode_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned()
}

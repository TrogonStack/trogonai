//! Replays what the upstream `bridge.mjs` did against a local server for each recorded case and checks that the Rust
//! worker client sends the same requests and reaches the same result
//! (`.config/mise/lib/claude-sdk-freshness/bridge_worker_goldens.mjs` writes the goldens).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use trogon_claude_agent_sdk::{
    AUTH_FAILURES_BEFORE_EXIT, DEFAULT_HEARTBEAT_INTERVAL_MS, HEARTBEAT_INTERVAL_CEILING_MS,
    HEARTBEAT_INTERVAL_FLOOR_MS, HEARTBEAT_TIMEOUT_CEILING_MS, HEARTBEAT_TIMEOUT_FLOOR_MS, HEARTBEAT_TIMEOUT_MARGIN_MS,
    InitError, InitFailure, InvalidSessionId, MismatchReason, NOT_FOUND_BEFORE_EXIT, NonOrigin403, RegisterFailure,
    RequestFailure, RequestOutcome, RequiresActionDetails, SensitiveString, SessionState, StatusClass, TokioPacing,
    WorkerClient, WorkerConfig, WorkerPacing, classify_status, external_metadata_patch, heartbeat_timeout,
    refusal_source, retry_after_ms, session_url, token_expiry,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");
const SESSION_PATH: &str = "/v1/code/sessions/cse_abc";
const SKIPPED_HEADERS: [&str; 5] = [
    "accept-encoding",
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
];
const ALL_REASONS: [MismatchReason; 8] = [
    MismatchReason::EpochConflict,
    MismatchReason::SupersededByWorker,
    MismatchReason::SessionNotActive,
    MismatchReason::OrganizationMovedAway,
    MismatchReason::EpochStale,
    MismatchReason::SessionNotFound,
    MismatchReason::TokenExpired,
    MismatchReason::AuthExhausted,
];

#[derive(Clone, Debug)]
struct Captured {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

type Log = Arc<Mutex<Vec<Captured>>>;

/// Records the pauses the worker asks for instead of waiting them out.
#[derive(Clone)]
struct Recorded {
    delays: Arc<Mutex<Vec<Duration>>>,
    jitter: f64,
}

impl Recorded {
    fn new(jitter: f64) -> Self {
        Self {
            delays: Arc::default(),
            jitter,
        }
    }

    fn delays(&self) -> Vec<Duration> {
        self.delays.lock().map(|delays| delays.clone()).unwrap_or_default()
    }
}

impl WorkerPacing for Recorded {
    fn jitter(&self) -> f64 {
        self.jitter
    }

    async fn pause(&self, delay: Duration) {
        if let Ok(mut delays) = self.delays.lock() {
            delays.push(delay);
        }
    }
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

async fn read_request(stream: &mut TcpStream) -> Option<Captured> {
    let mut bytes = Vec::new();
    let head_end = loop {
        if let Some(at) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let path = request_line.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_owned(), value.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = bytes[head_end + 4..].to_vec();
    while body.len() < length {
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some(Captured {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

async fn write_response(stream: &mut TcpStream, response: &Value, base: &str) -> TestResult {
    let status = field(response, "status").as_u64().unwrap_or(200);
    let body = field(response, "body").as_str().unwrap_or("");
    let mut head = format!("HTTP/1.1 {status} Status\r\n");
    let mut has_length = false;
    for pair in field(response, "headers").as_array().into_iter().flatten() {
        if let (Some(name), Some(value)) = (pair[0].as_str(), pair[1].as_str()) {
            has_length |= name.eq_ignore_ascii_case("content-length");
            head.push_str(&format!("{name}: {}\r\n", value.replace("{base}", base)));
        }
    }
    if !has_length {
        head.push_str(&format!("content-length: {}\r\n", body.len()));
    }
    head.push_str("connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

fn ok_response() -> Value {
    json!({"status": 200, "headers": [["content-type", "application/json"]], "body": "{}"})
}

fn next_response(queue: &[Value], taken: &mut usize) -> Value {
    let at = (*taken).min(queue.len().saturating_sub(1));
    *taken += 1;
    queue.get(at).cloned().unwrap_or_else(ok_response)
}

/// Serves the worker endpoints: each queue answers in order and then repeats its last answer. The string `"hang"`
/// holds the connection open without answering and `"destroy"` closes it at once.
async fn serve(register: Option<Value>, puts: Vec<Value>, beats: Vec<Value>) -> Result<(String, Log), std::io::Error> {
    serve_all(register, puts, beats, vec![]).await
}

/// `serve`, plus the answers `PUT /worker/landing` gives (where a redirect may point), in `{base}` header values.
async fn serve_all(
    register: Option<Value>,
    puts: Vec<Value>,
    beats: Vec<Value>,
    landings: Vec<Value>,
) -> Result<(String, Log), std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let origin = base.clone();
    let log: Log = Arc::default();
    let shared = log.clone();
    tokio::spawn(async move {
        let (mut put_count, mut beat_count, mut landing_count) = (0, 0, 0);
        let mut held = Vec::new();
        while let Ok((mut stream, _)) = listener.accept().await {
            let Some(mut request) = read_request(&mut stream).await else {
                continue;
            };
            request.path = request
                .path
                .strip_prefix(SESSION_PATH)
                .unwrap_or(&request.path)
                .to_owned();
            let route = format!("{} {}", request.method, request.path);
            let response = match route.as_str() {
                "POST /worker/register" => register
                    .clone()
                    .unwrap_or_else(|| json!({"status": 200, "headers": [], "body": "{\"worker_epoch\":1}"})),
                "PUT /worker" => next_response(&puts, &mut put_count),
                "PUT /worker/landing" => next_response(&landings, &mut landing_count),
                "POST /worker/heartbeat" => next_response(&beats, &mut beat_count),
                _ => json!({"status": 404, "headers": [], "body": ""}),
            };
            if let Ok(mut captured) = shared.lock() {
                captured.push(request);
            }
            if response == "hang" {
                held.push(stream);
                continue;
            }
            if response == "destroy" {
                drop(stream);
                continue;
            }
            let _ = write_response(&mut stream, &response, &origin).await;
        }
    });
    Ok((base, log))
}

async fn closed_port() -> Result<String, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    Ok(format!("http://{}", listener.local_addr()?))
}

fn config(base: &str, token: &str) -> WorkerConfig {
    WorkerConfig {
        api_base_url: base.to_owned(),
        session_id: "cse_abc".to_owned(),
        token: SensitiveString::from(token),
        entrypoint: None,
        heartbeat_interval_ms: None,
        session_access_token: None,
        non_origin_403: NonOrigin403::default(),
    }
}

fn client(base: &str, pacing: Recorded) -> Result<WorkerClient<reqwest::Client, Recorded>, InvalidSessionId> {
    WorkerClient::new(reqwest::Client::new(), pacing, config(base, "tok"))
}

fn sorted_headers(headers: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut kept: Vec<(String, String)> = headers
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .filter(|(name, _)| !SKIPPED_HEADERS.contains(&name.as_str()))
        .collect();
    kept.sort();
    kept
}

fn assert_request(name: &str, expected: &Value, actual: &Captured) {
    assert_eq!(
        text(field(expected, "method")).as_deref(),
        Some(actual.method.as_str()),
        "{name} method"
    );
    assert_eq!(
        text(field(expected, "path")).as_deref(),
        Some(actual.path.as_str()),
        "{name} path"
    );
    assert_eq!(
        text(field(expected, "body")).as_deref(),
        Some(actual.body.as_str()),
        "{name} body"
    );
    let want = sorted_headers(
        field(expected, "headers")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|pair| Some((pair[0].as_str()?.to_owned(), pair[1].as_str()?.to_owned()))),
    );
    assert_eq!(want, sorted_headers(actual.headers.iter().cloned()), "{name} headers");
}

fn queue(case: &Value, key: &str) -> Vec<Value> {
    field(case, key).as_array().cloned().unwrap_or_default()
}

fn state_named(name: &str) -> Result<SessionState, String> {
    match name {
        "idle" => Ok(SessionState::Idle),
        "running" => Ok(SessionState::Running),
        "requires_action" => Ok(SessionState::RequiresAction),
        other => Err(format!("unknown state {other}")),
    }
}

fn reason_named(name: &str) -> Result<MismatchReason, String> {
    ALL_REASONS
        .into_iter()
        .find(|reason| reason.as_str() == name)
        .ok_or_else(|| format!("unknown reason {name}"))
}

/// What the attach entrypoint closes with: init failures the service refused for good close with `4091`, every
/// terminal request-path condition with `4090`.
struct Ending {
    closes: Vec<u16>,
    reason: Option<&'static str>,
}

impl Ending {
    fn mismatch(reason: MismatchReason) -> Self {
        Self {
            closes: vec![reason.close_code(false)],
            reason: Some(reason.as_str()),
        }
    }
}

async fn run_case(case: &Value) -> TestResult {
    let name = text(field(case, "name")).ok_or("unnamed case")?;
    let register = case.get("register").filter(|response| !response.is_null()).cloned();
    let (base, log) = serve_all(
        register.clone(),
        queue(case, "puts"),
        queue(case, "beat"),
        queue(case, "landing"),
    )
    .await?;
    let mut worker = WorkerClient::new(
        reqwest::Client::new(),
        Recorded::new(0.0),
        WorkerConfig {
            session_access_token: text(field(case, "session_token")).map(SensitiveString::from),
            ..config(&base, "tok")
        },
    )?;
    let mut epoch = field(case, "epoch_option").as_i64();
    let mut ending = Ending {
        closes: Vec::new(),
        reason: None,
    };
    let mut connected = false;
    let mut registered = true;
    if register.is_some() {
        match worker.register_worker().await {
            Ok(registered_epoch) => {
                assert!(field(case, "rejection").is_null(), "{name} should have been rejected");
                epoch = Some(registered_epoch);
            }
            Err(failure) => {
                let rejection = field(case, "rejection");
                assert_eq!(
                    Some(failure.message()),
                    text(field(rejection, "message")),
                    "{name} message"
                );
                assert_eq!(failure.code(), field(rejection, "code").as_str(), "{name} code");
                registered = false;
            }
        }
    }
    if registered {
        match worker.initialize(epoch.ok_or("no epoch")?).await {
            Ok(()) => connected = true,
            Err(InitError::Failed(failure)) => {
                assert!(
                    matches!(failure.http_status, Some(400 | 413 | 422)),
                    "{name} unexpected init failure {failure:?}"
                );
                ending.closes.push(4091);
            }
            Err(InitError::Mismatch(reason)) => ending = Ending::mismatch(reason),
        }
    }
    if connected {
        for step in field(case, "steps").as_array().into_iter().flatten() {
            let outcome = match field(step, "op").as_str() {
                Some("state") => {
                    let state = state_named(field(step, "state").as_str().unwrap_or_default())?;
                    worker.report_state(state, None).await.map(|sent| sent.is_some())
                }
                _ => {
                    let metadata = field(step, "value").as_object().cloned().unwrap_or_default();
                    worker.report_metadata(&metadata).await.map(|_| true)
                }
            };
            match outcome {
                Ok(sent) => assert_eq!(Some(sent), field(step, "sends").as_bool(), "{name} step {step}"),
                Err(reason) => {
                    ending = Ending::mismatch(reason);
                    break;
                }
            }
        }
    }
    if connected && ending.closes.is_empty() {
        for _ in 0..field(case, "beats").as_u64().unwrap_or(0) {
            if let Err(reason) = worker.heartbeat().await {
                ending = Ending::mismatch(reason);
                break;
            }
        }
    }
    if let Some(expected) = field(case, "connected").as_bool() {
        assert_eq!(connected, expected, "{name} connected");
    }
    let closes: Vec<u16> = queue(case, "closes")
        .iter()
        .filter_map(|code| code.as_u64().and_then(|code| u16::try_from(code).ok()))
        .collect();
    assert_eq!(ending.closes, closes, "{name} closes");
    assert_eq!(ending.reason, field(case, "reason").as_str(), "{name} reason");
    if let Some(expected) = field(case, "epoch").as_i64() {
        assert_eq!(worker.epoch(), expected, "{name} epoch");
    }
    let expected = queue(case, "requests");
    let captured = log.lock().map_err(|_| "poisoned log")?;
    assert_eq!(captured.len(), expected.len(), "{name} request count");
    for (position, (want, got)) in expected.iter().zip(captured.iter()).enumerate() {
        assert_request(&format!("{name} #{position}"), want, got);
    }
    Ok(())
}

async fn replay(section: &str, minimum: usize) -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let cases = goldens[section].as_array().ok_or("missing section")?;
    assert!(
        cases.len() >= minimum,
        "the {section} corpus shrank to {} cases",
        cases.len()
    );
    for case in cases {
        run_case(case).await?;
    }
    Ok(())
}

#[tokio::test]
async fn register_worker_matches_the_upstream_client() -> TestResult {
    replay("register", 28).await
}

#[tokio::test]
async fn init_matches_the_upstream_client() -> TestResult {
    replay("init", 8).await
}

#[tokio::test]
async fn state_and_metadata_reports_match_the_upstream_client() -> TestResult {
    replay("state", 6).await
}

#[tokio::test]
async fn heartbeats_match_the_upstream_client() -> TestResult {
    replay("heartbeat", 28).await
}

#[tokio::test]
async fn token_expiry_and_refusal_sources_match_the_upstream_client() -> TestResult {
    replay("auth", 14).await
}

#[test]
fn pure_rules_match_the_upstream_client() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let pure = &goldens["pure"];
    for case in pure["session_urls"].as_array().ok_or("no session urls")? {
        let built = session_url(
            field(case, "base").as_str().unwrap_or_default(),
            field(case, "id").as_str().unwrap_or_default(),
        );
        match built {
            Ok(url) => assert_eq!(Some(url), text(field(case, "url")), "{case}"),
            Err(error) => assert_eq!(Some(error.to_string()), text(field(case, "error")), "{case}"),
        }
    }
    for case in pure["retry_after"].as_array().ok_or("no retry after")? {
        let header = field(case, "header").as_str().unwrap_or_default();
        assert_eq!(retry_after_ms(header), field(case, "ms").as_f64(), "{case}");
    }
    let constants = &pure["constants"];
    for (key, value) in [
        ("heartbeat_interval_ms", DEFAULT_HEARTBEAT_INTERVAL_MS),
        ("heartbeat_interval_floor_ms", HEARTBEAT_INTERVAL_FLOOR_MS),
        ("heartbeat_interval_ceiling_ms", HEARTBEAT_INTERVAL_CEILING_MS),
        ("heartbeat_timeout_ceiling_ms", HEARTBEAT_TIMEOUT_CEILING_MS),
        ("heartbeat_timeout_margin_ms", HEARTBEAT_TIMEOUT_MARGIN_MS),
        ("heartbeat_timeout_floor_ms", HEARTBEAT_TIMEOUT_FLOOR_MS),
        ("auth_failures_before_exit", u64::from(AUTH_FAILURES_BEFORE_EXIT)),
        ("not_found_before_exit", u64::from(NOT_FOUND_BEFORE_EXIT)),
    ] {
        assert_eq!(constants[key].as_u64(), Some(value), "{key}");
    }
    for (name, code) in pure["close_codes"].as_object().ok_or("no close codes")? {
        let reason = reason_named(name)?;
        assert_eq!(code.as_u64(), Some(u64::from(reason.close_code(true))), "{name}");
        assert_eq!(reason.close_code(false), 4090, "{name} untyped");
    }
    for (name, retryable) in pure["retryable"].as_object().ok_or("no retryable")? {
        assert_eq!(retryable.as_bool(), Some(reason_named(name)?.retryable()), "{name}");
    }
    for pair in pure["classify_status"].as_array().ok_or("no classify")? {
        let status = pair[0].as_u64().and_then(|status| u16::try_from(status).ok());
        let class = match classify_status(status) {
            StatusClass::Indeterminate => "indeterminate",
            StatusClass::NotAccepted => "not_accepted",
        };
        assert_eq!(pair[1].as_str(), Some(class), "{pair}");
    }
    for pair in pure["conflict_reasons"].as_array().ok_or("no conflict reasons")? {
        let reason = pair[0].as_str().and_then(MismatchReason::from_conflict);
        assert_eq!(reason.map(MismatchReason::as_str), pair[1].as_str(), "{pair}");
    }
    for case in pure["requires_action"].as_array().ok_or("no requires action")? {
        let input = &case["input"];
        let required = |key: &str| text(field(input, key)).unwrap_or_default();
        let details = RequiresActionDetails {
            tool_name: required("tool_name"),
            display_tool_name: text(field(input, "display_tool_name")),
            action_description: required("action_description"),
            raw_command: text(field(input, "raw_command")),
            request_id: required("request_id"),
            tool_use_id: required("tool_use_id"),
        };
        assert_eq!(details.to_json(), case["output"], "{case}");
    }
    for case in pure["metadata"].as_array().ok_or("no metadata")? {
        let input: Map<String, Value> = case["input"].as_object().cloned().ok_or("metadata input")?;
        assert_eq!(Value::Object(external_metadata_patch(&input)), case["output"], "{case}");
    }
    Ok(())
}

/// Answers every lookup as a missing host, so a name fails the way it does without a network.
struct NoSuchHost;

impl reqwest::dns::Resolve for NoSuchHost {
    fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async {
            let missing: Box<dyn std::error::Error + Send + Sync> =
                Box::new(std::io::Error::new(std::io::ErrorKind::NotFound, "no such host"));
            Err(missing)
        })
    }
}

#[tokio::test]
async fn a_failure_with_no_node_equivalent_is_a_network_error() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let tls = format!("https://{}", listener.local_addr()?);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
            let _ = stream.shutdown().await;
        }
    });
    let failure = client(&tls, Recorded::new(0.0))?
        .register_worker()
        .await
        .err()
        .ok_or("expected a failure")?;
    assert_eq!(failure.message(), "Network Error".to_owned());
    assert_eq!(failure.code(), Some("ERR_NETWORK"));
    Ok(())
}

#[tokio::test]
async fn register_failures_carry_the_node_error_text() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let cases = goldens["network"].as_array().ok_or("no network cases")?;
    assert!(cases.len() >= 8, "the network corpus shrank");
    for case in cases {
        let mode = field(case, "mode").as_str().unwrap_or_default();
        let mut base = String::new();
        let mut register = None;
        match mode {
            "refused" => base = closed_port().await?,
            "invalid" => {}
            "localhost" => base = closed_port().await?.replace("127.0.0.1", "localhost"),
            "notfound" => {
                let failure = WorkerClient::new(
                    reqwest::Client::builder().dns_resolver(Arc::new(NoSuchHost)).build()?,
                    Recorded::new(0.0),
                    config("http://nonexistent.invalid", "tok"),
                )?
                .register_worker()
                .await
                .err()
                .ok_or("expected a failure")?;
                assert_eq!(
                    Some(failure.message()),
                    text(field(case, "message")).as_deref(),
                    "notfound message"
                );
                assert_eq!(failure.code(), field(case, "code").as_str(), "notfound code");
                continue;
            }
            "ftp" => base = "ftp://h".to_owned(),
            "destroy" => register = Some(json!("destroy")),
            "loop" => {
                let location = json!([["location", "{base}/v1/code/sessions/cse_abc/worker/register"]]);
                register = Some(json!({"status": 302, "headers": location, "body": ""}));
            }
            "truncated" => {
                let headers = json!([["content-length", "100"], ["content-type", "application/json"]]);
                register = Some(json!({"status": 200, "headers": headers, "body": "{\"wor"}));
            }
            other => return Err(format!("unknown network mode {other}").into()),
        }
        if register.is_some() {
            base = serve(register, vec![], vec![]).await?.0;
        }
        let failure = client(&base, Recorded::new(0.0))?
            .register_worker()
            .await
            .err()
            .ok_or("expected a failure")?;
        let port = base.rsplit(':').next().unwrap_or_default();
        let expected = text(field(case, "message")).unwrap_or_default().replace("<port>", port);
        assert_eq!(failure.message(), expected, "{mode} message");
        assert_eq!(failure.code(), field(case, "code").as_str(), "{mode} code");
    }
    Ok(())
}

#[tokio::test]
async fn requests_that_fail_before_headers_report_the_fetch_error() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    for case in goldens["fetch_failures"].as_array().ok_or("no fetch failures")? {
        let mode = field(case, "mode").as_str().unwrap_or_default();
        if mode == "hang" {
            continue;
        }
        let answer = if mode == "truncated" {
            json!({"status": 200, "headers": [["content-length", "10"]], "body": "ab"})
        } else {
            json!(mode)
        };
        let (base, _) = serve(None, vec![], vec![answer]).await?;
        let mut worker = client(&base, Recorded::new(0.0))?;
        let outcome = worker.heartbeat().await;
        match field(case, "reason").as_str() {
            Some(reason) => match outcome {
                Ok(RequestOutcome::Failed(failure)) => assert_eq!(failure.reason, reason, "{mode}"),
                other => return Err(format!("{mode}: {other:?}").into()),
            },
            None => assert_eq!(outcome, Ok(RequestOutcome::Accepted(None)), "{mode}"),
        }
    }
    Ok(())
}

#[test]
fn refusal_sources_and_token_expiry_match_the_upstream_client() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let pure = &goldens["pure"];
    for case in pure["refusal_sources"].as_array().ok_or("no refusal sources")? {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in field(case, "headers").as_object().ok_or("no headers")? {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
                reqwest::header::HeaderValue::from_str(value.as_str().unwrap_or_default())?,
            );
        }
        let source = refusal_source(&headers);
        assert_eq!(Some(source.as_str()), field(case, "source").as_str(), "{case}");
        assert_eq!(
            Some(source.is_non_origin()),
            field(case, "non_origin").as_bool(),
            "{case}"
        );
    }
    for case in pure["token_expiry"].as_array().ok_or("no token expiry")? {
        let token = field(case, "token").as_str().unwrap_or_default();
        assert_eq!(token_expiry(token), field(case, "exp").as_f64(), "{case}");
    }
    Ok(())
}

#[tokio::test]
async fn held_tickets_follow_the_token_in_use() -> TestResult {
    let ticketed = json!({"status": 200, "headers": [["x-ccr-ticket-session", "t1"]], "body": "{}"});
    let (base, _) = serve(None, vec![], vec![ticketed]).await?;
    let mut worker = client(&base, Recorded::new(0.0))?;
    worker.heartbeat().await.map_err(|reason| reason.as_str())?;
    assert_eq!(worker.session_ticket(), Some("t1"));
    worker.set_token(SensitiveString::from("other"));
    assert_eq!(worker.session_ticket(), None);
    worker.set_session_access_token(Some(SensitiveString::from("h.e30.s")));
    Ok(())
}

#[tokio::test]
async fn a_transient_non_origin_403_never_exhausts_the_session() -> TestResult {
    let refusal = json!({"status": 403, "headers": [["cf-ray", "abc-LHR"]], "body": ""});
    let (base, _) = serve(None, vec![], vec![refusal]).await?;
    let mut worker = WorkerClient::new(
        reqwest::Client::new(),
        Recorded::new(0.0),
        WorkerConfig {
            non_origin_403: NonOrigin403::Transient,
            ..config(&base, "tok")
        },
    )?;
    for _ in 0..(AUTH_FAILURES_BEFORE_EXIT + 2) {
        match worker.heartbeat().await {
            Ok(RequestOutcome::Failed(failure)) => assert!(failure.non_origin_refusal),
            other => return Err(format!("{other:?}").into()),
        }
    }
    Ok(())
}

#[test]
fn heartbeat_timeout_leaves_room_for_the_next_beat() {
    assert_eq!(heartbeat_timeout(20_000.0), Duration::from_millis(15_000));
    assert_eq!(heartbeat_timeout(12_000.0), Duration::from_millis(7_000));
    assert_eq!(heartbeat_timeout(30.0), Duration::from_millis(5_000));
    assert_eq!(heartbeat_timeout(300_000.0), Duration::from_millis(15_000));
}

#[test]
fn invalid_session_ids_are_refused() {
    let refused = WorkerClient::new(
        reqwest::Client::new(),
        Recorded::new(0.0),
        WorkerConfig {
            session_id: "a/b".to_owned(),
            ..config("http://h", "tok")
        },
    );
    assert_eq!(refused.err(), Some(InvalidSessionId));
}

#[test]
fn a_new_client_starts_idle_with_the_default_cadence() -> TestResult {
    let worker = client("http://h/", Recorded::new(0.0))?;
    assert_eq!(worker.session_url(), "http://h/v1/code/sessions/cse_abc");
    assert_eq!(worker.epoch(), 0);
    assert_eq!(worker.state(), SessionState::Idle);
    assert!((worker.heartbeat_interval_ms() - 20_000.0).abs() < f64::EPSILON);
    let set = WorkerClient::new(
        reqwest::Client::new(),
        Recorded::new(0.0),
        WorkerConfig {
            heartbeat_interval_ms: Some(1234),
            ..config("http://h", "tok")
        },
    )?;
    assert!((set.heartbeat_interval_ms() - 1234.0).abs() < f64::EPSILON);
    Ok(())
}

#[tokio::test]
async fn init_backs_off_between_attempts_and_then_gives_up() -> TestResult {
    let (base, log) = serve(None, vec![json!({"status": 500, "headers": [], "body": ""})], vec![]).await?;
    let pacing = Recorded::new(0.0);
    let mut worker = client(&base, pacing.clone())?;
    let failure = InitFailure {
        reason: "worker_register_failed",
        http_status: Some(500),
        no_answer: false,
    };
    assert_eq!(worker.initialize(7).await, Err(InitError::Failed(failure.clone())));
    assert_eq!(failure.to_string(), "CCRClient init failed: worker_register_failed");
    let expected: Vec<Duration> = [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]
        .into_iter()
        .map(Duration::from_millis)
        .collect();
    assert_eq!(pacing.delays(), expected);
    assert_eq!(log.lock().map_err(|_| "poisoned log")?.len(), 10);
    Ok(())
}

#[tokio::test]
async fn init_retries_with_jitter_until_the_service_accepts() -> TestResult {
    let (base, log) = serve(
        None,
        vec![json!({"status": 503, "headers": [], "body": ""}), ok_response()],
        vec![],
    )
    .await?;
    let pacing = Recorded::new(0.5);
    let mut worker = client(&base, pacing.clone())?;
    assert_eq!(worker.initialize(7).await, Ok(()));
    assert_eq!(pacing.delays(), vec![Duration::from_millis(750)]);
    assert_eq!(log.lock().map_err(|_| "poisoned log")?.len(), 2);
    assert_eq!(worker.epoch(), 7);
    Ok(())
}

#[tokio::test]
async fn init_reports_that_nobody_answered() -> TestResult {
    let mut worker = client(&closed_port().await?, Recorded::new(0.0))?;
    assert_eq!(
        worker.initialize(1).await,
        Err(InitError::Failed(InitFailure {
            reason: "worker_register_failed",
            http_status: None,
            no_answer: true,
        }))
    );
    Ok(())
}

#[tokio::test]
async fn a_client_error_clears_the_held_tickets_but_a_429_does_not() -> TestResult {
    let ticket = json!({"status": 200, "headers": [["x-ccr-ticket-rev", "rev1"]], "body": "{}"});
    for (status, kept) in [(400, false), (429, true)] {
        let refusal = json!({"status": status, "headers": [], "body": ""});
        let (base, log) = serve(None, vec![ticket.clone(), refusal, ok_response()], vec![]).await?;
        let mut worker = client(&base, Recorded::new(0.0))?;
        worker.initialize(1).await.map_err(|_| "init failed")?;
        let refused = worker.report_state(SessionState::Running, None).await;
        assert!(matches!(refused, Ok(Some(RequestOutcome::Failed(_)))), "{status}");
        worker
            .report_metadata(&Map::new())
            .await
            .map_err(|_| "metadata failed")?;
        let captured = log.lock().map_err(|_| "poisoned log")?;
        let has_ticket = |at: usize| {
            captured[at]
                .headers
                .iter()
                .any(|(name, value)| name == "x-ccr-ticket-rev" && value == "rev1")
        };
        assert!(!has_ticket(0), "{status}");
        assert!(has_ticket(1), "{status}");
        assert_eq!(has_ticket(2), kept, "{status}");
    }
    Ok(())
}

#[tokio::test]
async fn register_worker_fails_when_the_request_cannot_be_made() -> TestResult {
    let worker = client("", Recorded::new(0.0))?;
    let failure = worker.register_worker().await.err().ok_or("expected a failure")?;
    assert_eq!(failure.to_string(), "Invalid URL");
    assert_eq!(failure.code(), Some("ERR_INVALID_URL"));
    let wide_token = WorkerClient::new(
        reqwest::Client::new(),
        Recorded::new(0.0),
        config("http://h", "a\u{100}"),
    )?;
    let failure = wide_token.register_worker().await.err().ok_or("expected a failure")?;
    assert_eq!(
        failure.to_string(),
        "Invalid character in header content [\"Authorization\"]"
    );
    assert_eq!(failure.code(), Some("ERR_INVALID_CHAR"));
    Ok(())
}

#[tokio::test]
async fn register_worker_reads_a_body_that_starts_with_a_byte_order_mark() -> TestResult {
    let body = json!({"status": 200, "headers": [], "body": "\u{feff}{\"worker_epoch\":2}"});
    let (base, _) = serve(Some(body), vec![], vec![]).await?;
    assert_eq!(client(&base, Recorded::new(0.0))?.register_worker().await, Ok(2));
    Ok(())
}

#[tokio::test]
async fn requests_that_cannot_be_made_do_not_reach_the_service() -> TestResult {
    let mut bad_token = WorkerClient::new(
        reqwest::Client::new(),
        Recorded::new(0.0),
        config("http://h", "a\u{100}"),
    )?;
    let outcome = bad_token.report_metadata(&Map::new()).await;
    assert_eq!(outcome, Ok(type_error()));
    let mut relative = client("", Recorded::new(0.0))?;
    assert_eq!(relative.heartbeat().await, Ok(type_error()));
    let mut refused = client(&closed_port().await?, Recorded::new(0.0))?;
    assert_eq!(refused.heartbeat().await, Ok(type_error()));
    Ok(())
}

fn type_error() -> RequestOutcome {
    RequestOutcome::Failed(RequestFailure {
        status: None,
        reason: "fetch_failed:TypeError".to_owned(),
        retry_after_ms: None,
        fetch_failed: true,
        non_origin_refusal: false,
    })
}

#[tokio::test]
async fn a_body_that_ends_early_is_accepted_without_its_data() -> TestResult {
    let truncated = json!({"status": 200, "headers": [["content-length", "10"]], "body": "ab"});
    let (base, _) = serve(None, vec![], vec![truncated]).await?;
    let mut worker = client(&base, Recorded::new(0.0))?;
    assert_eq!(worker.heartbeat().await, Ok(RequestOutcome::Accepted(None)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_silent_service_times_the_request_out() -> TestResult {
    let (base, _) = serve(Some(json!("hang")), vec![], vec![json!("hang")]).await?;
    let mut worker = client(&base, Recorded::new(0.0))?;
    let failure = worker.register_worker().await.err().ok_or("expected a failure")?;
    assert_eq!(failure, RegisterFailure::TimedOut);
    assert_eq!(failure.to_string(), "timeout of 10000ms exceeded");
    assert_eq!(failure.code(), Some("ECONNABORTED"));
    let outcome = worker.heartbeat().await;
    assert_eq!(
        outcome,
        Ok(RequestOutcome::Failed(RequestFailure {
            status: None,
            reason: "fetch_failed:TimeoutError".to_owned(),
            retry_after_ms: None,
            fetch_failed: true,
            non_origin_refusal: false,
        }))
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_default_pacing_waits_on_the_timer() {
    let pacing = TokioPacing;
    assert!((0.0..1.0).contains(&pacing.jitter()));
    pacing.pause(Duration::from_millis(5)).await;
}

#[tokio::test]
async fn heartbeat_answers_move_the_beat_interval_within_its_bounds() -> TestResult {
    let answers = ["30", "0.01", "100000", "0", "-4", "\"7\"", "null"]
        .map(|seconds| json!({"status": 200, "headers": [], "body": format!("{{\"heartbeat_interval_seconds\":{seconds}}}")}));
    let (base, _) = serve(None, vec![], answers.to_vec()).await?;
    let mut worker = client(&base, Recorded::new(0.0))?;
    for expected in [
        30_000.0, 10_000.0, 300_000.0, 300_000.0, 300_000.0, 300_000.0, 300_000.0,
    ] {
        let outcome = worker.heartbeat().await.map_err(|_| "heartbeat refused")?;
        assert!(outcome.is_accepted());
        assert!((worker.heartbeat_interval_ms() - expected).abs() < f64::EPSILON);
    }
    Ok(())
}

#[tokio::test]
async fn a_429_reports_how_long_the_service_asked_to_wait() -> TestResult {
    let answer = |status: u16, retry_after: Option<&str>| {
        let headers: Vec<Value> = retry_after
            .map(|value| json!(["retry-after", value]))
            .into_iter()
            .collect();
        json!({"status": status, "headers": headers, "body": ""})
    };
    let answers = vec![
        answer(429, Some("3")),
        answer(429, None),
        answer(429, Some("-1")),
        answer(500, Some("3")),
    ];
    let (base, _) = serve(None, vec![], answers).await?;
    let mut worker = client(&base, Recorded::new(0.0))?;
    let mut waits = Vec::new();
    for _ in 0..4 {
        match worker.heartbeat().await {
            Ok(RequestOutcome::Failed(failure)) => waits.push(failure.retry_after_ms),
            other => return Err(format!("unexpected {other:?}").into()),
        }
    }
    assert_eq!(waits, vec![Some(3000.0), None, None, None]);
    Ok(())
}

#[tokio::test]
async fn requires_action_details_ride_along_and_always_send() -> TestResult {
    let (base, log) = serve(None, vec![], vec![]).await?;
    let mut worker = client(&base, Recorded::new(0.0))?;
    worker.initialize(5).await.map_err(|_| "init failed")?;
    let details = RequiresActionDetails {
        tool_name: "Bash".to_owned(),
        display_tool_name: None,
        action_description: "run it".to_owned(),
        raw_command: Some("ls".to_owned()),
        request_id: "r".to_owned(),
        tool_use_id: "t".to_owned(),
    };
    for _ in 0..2 {
        let sent = worker.report_state(SessionState::RequiresAction, Some(&details)).await;
        assert!(matches!(sent, Ok(Some(RequestOutcome::Accepted(None)))));
    }
    assert_eq!(worker.state(), SessionState::RequiresAction);
    let captured = log.lock().map_err(|_| "poisoned log")?;
    let want = "{\"worker_epoch\":5,\"worker_status\":\"requires_action\",\"requires_action_details\":{\"tool_name\":\"Bash\",\"action_description\":\"run it\",\"raw_command\":\"ls\",\"request_id\":\"r\",\"tool_use_id\":\"t\"},\"internal_metadata\":{\"reply_degraded\":false}}";
    assert_eq!(captured.len(), 3);
    assert_eq!(captured[1].body, want);
    assert_eq!(captured[2].body, want);
    Ok(())
}

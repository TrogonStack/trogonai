//! Replays the recorded `stream` and `attach` cases (written by
//! `.config/mise/lib/claude-sdk-freshness/bridge_worker_goldens.mjs` against the real `attachBridgeSession`) through
//! [`BridgeSession`]: the requests it sends to a local server and what it reports to the host must match what the
//! upstream entrypoint did.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use trogon_claude_agent_sdk::{
    AttachBridgeSessionOptions, AttachCallbacks, AttachError, BridgeSession, CLOSE_INIT_FAILED, EnvironmentLookup,
    Reopen, StreamOpen, StreamRequest, WorkerPacing, attach_bridge_session,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Session = BridgeSession<reqwest::Client, Recorded>;
type Shared<T> = Arc<Mutex<Vec<T>>>;

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");
const SESSION_PATH: &str = "/v1/code/sessions/cse_abc";
const STREAM_PATH: &str = "/worker/events/stream";
const SKIPPED_HEADERS: [&str; 5] = [
    "accept-encoding",
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
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
struct Recorded;

impl WorkerPacing for Recorded {
    fn jitter(&self) -> f64 {
        0.0
    }

    async fn pause(&self, _delay: Duration) {}
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

fn text(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

fn number(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

fn chunk_bytes(chunk: &Value) -> Result<Vec<u8>, String> {
    if let Some(text) = chunk.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    list(chunk, "bytes")?
        .iter()
        .map(|byte| {
            byte.as_u64()
                .and_then(|byte| u8::try_from(byte).ok())
                .ok_or_else(|| format!("bad byte in {chunk}"))
        })
        .collect()
}

fn milliseconds(delay: Duration) -> u64 {
    (delay.as_secs_f64() * 1000.0).round() as u64
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

async fn write_response(stream: &mut TcpStream, response: &Value) -> TestResult {
    let status = field(response, "status").as_u64().unwrap_or(200);
    let body = field(response, "body").as_str().unwrap_or("");
    let mut head = format!("HTTP/1.1 {status} Status\r\n");
    for pair in field(response, "headers").as_array().into_iter().flatten() {
        if let (Some(name), Some(value)) = (pair[0].as_str(), pair[1].as_str()) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    head.push_str(&format!("content-length: {}\r\nconnection: close\r\n\r\n", body.len()));
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

fn queue(case: &Value, key: &str) -> Vec<Value> {
    field(case, key).as_array().cloned().unwrap_or_default()
}

fn next_response(queue: &[Value], taken: &mut usize) -> Value {
    let at = (*taken).min(queue.len().saturating_sub(1));
    *taken += 1;
    queue
        .get(at)
        .cloned()
        .unwrap_or_else(|| json!({"status": 200, "headers": [], "body": "{}"}))
}

/// Serves the worker endpoints of a case: each queue answers in order and then repeats its last answer. The read
/// stream is not served, the tests feed it to the session themselves.
async fn serve(case: &Value) -> Result<(String, Log), std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let log: Log = Arc::default();
    let shared = log.clone();
    let register = case
        .get("register")
        .filter(|response| !response.is_null())
        .cloned()
        .unwrap_or_else(|| json!({"status": 200, "headers": [], "body": "{\"worker_epoch\":5}"}));
    let (puts, beats, events, deliveries) = (
        queue(case, "puts"),
        queue(case, "beat"),
        queue(case, "events"),
        queue(case, "deliveries"),
    );
    tokio::spawn(async move {
        let (mut put_count, mut beat_count, mut event_count, mut delivery_count) = (0, 0, 0, 0);
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
                "POST /worker/register" => register.clone(),
                "PUT /worker" => next_response(&puts, &mut put_count),
                "POST /worker/heartbeat" => next_response(&beats, &mut beat_count),
                "POST /worker/events" => next_response(&events, &mut event_count),
                "POST /worker/events/delivery" => next_response(&deliveries, &mut delivery_count),
                _ => json!({"status": 404, "headers": [], "body": ""}),
            };
            if let Ok(mut captured) = shared.lock() {
                captured.push(request);
            }
            let _ = write_response(&mut stream, &response).await;
        }
    });
    Ok((base, log))
}

fn http() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(21))
        .referer(false)
        .build()
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
        Some(actual.method.as_str()),
        field(expected, "method").as_str(),
        "{name} method"
    );
    assert_eq!(
        Some(actual.path.as_str()),
        field(expected, "path").as_str(),
        "{name} path"
    );
    assert_eq!(
        Some(actual.body.as_str()),
        field(expected, "body").as_str(),
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

/// What the host and the server saw of one session.
#[derive(Debug, Default, PartialEq)]
struct Observed {
    gets: Vec<StreamRequest>,
    delays: Vec<u64>,
    closes: Vec<u16>,
    acks: Vec<(String, String)>,
    inbound: Vec<Value>,
    permission_responses: Vec<Value>,
    refused_requests: Vec<String>,
    probes: u64,
    last_sequence: i64,
}

struct Hosted {
    closes: Shared<u16>,
    inbound: Shared<Value>,
    permissions: Shared<Value>,
}

fn pushed<T>(list: &Shared<T>, value: T) {
    if let Ok(mut list) = list.lock() {
        list.push(value);
    }
}

fn taken<T: Clone>(list: &Shared<T>) -> Vec<T> {
    list.lock().map(|list| list.clone()).unwrap_or_default()
}

fn options(case: &Value, base: &str) -> Result<(AttachBridgeSessionOptions, Hosted), Box<dyn std::error::Error>> {
    let hosted = Hosted {
        closes: Arc::default(),
        inbound: Arc::default(),
        permissions: Arc::default(),
    };
    let session_id = field(case, "session_id").as_str().unwrap_or("cse_abc");
    let mut options = AttachBridgeSessionOptions::new(session_id, base, "tok");
    options.epoch = field(case, "epoch_option").as_i64();
    options.initial_sequence_num = field(case, "initial_sequence_num").as_i64();
    options.heartbeat_interval_ms = Some(number(case, "interval_ms")?);
    options.outbound_only = case.get("stream").is_none();
    options.arguments = Some(Vec::new());
    let token = field(case, "session_token").as_str().map(str::to_owned);
    let environment: EnvironmentLookup = Arc::new(move |name| {
        (name == "CLAUDE_CODE_SESSION_ACCESS_TOKEN")
            .then(|| token.clone())
            .flatten()
    });
    options.environment = Some(environment);
    let (closes, inbound, permissions) = (
        hosted.closes.clone(),
        hosted.inbound.clone(),
        hosted.permissions.clone(),
    );
    options.callbacks = AttachCallbacks {
        on_inbound_message: Some(Box::new(move |message| pushed(&inbound, message))),
        on_permission_response: Some(Box::new(move |message| {
            pushed(&permissions, message.clone());
            Some(true)
        })),
        on_close: Some(Box::new(move |code| pushed(&closes, code))),
        ..AttachCallbacks::default()
    };
    Ok((options, hosted))
}

async fn attach(
    case: &Value,
    base: &str,
) -> Result<(Result<Session, AttachError>, Hosted), Box<dyn std::error::Error>> {
    let (options, hosted) = options(case, base)?;
    Ok((attach_bridge_session(http()?, Recorded, options).await, hosted))
}

fn requests_of(log: &Log) -> Vec<Captured> {
    log.lock().map(|log| log.clone()).unwrap_or_default()
}

/// Feeds the scripted connections of a case to the session the way the upstream reader read them.
async fn read_stream(case: &Value, session: &mut Session, observed: &mut Observed) -> TestResult {
    let mut timed_out = false;
    for connection in list(case, "stream")? {
        let Some(request) = session.stream_request() else {
            break;
        };
        observed.gets.push(request);
        match session.stream_opened(u16::try_from(number(connection, "status")?)?) {
            StreamOpen::Close(_) => break,
            StreamOpen::Retry => {
                observed.delays.push(milliseconds(session.stream_lost(0.5)));
                continue;
            }
            StreamOpen::Read => {}
        }
        for chunk in list(connection, "chunks")? {
            session.stream_push(&chunk_bytes(chunk)?).await;
            session.settle_control().await;
            session.pump().await;
        }
        if text(connection, "then")? == "end" {
            observed.delays.push(milliseconds(session.stream_lost(0.5)));
        } else if !timed_out && let Some(clock) = case.get("clock") {
            timed_out = true;
            // The recorder bent the clocks: the liveness timer fired after 300 ms of real time, and a clock that
            // jumped reads 45 s plus its lateness, one that did not reads the 300 ms.
            let silence = |late: &Value| late.as_u64().map_or(300, |late| 45_000 + late);
            let wall = Duration::from_millis(silence(&clock["late_wall"]));
            let monotonic = Duration::from_millis(silence(&clock["late_mono"]));
            match session.stream_silent(wall, monotonic, 0.5) {
                Reopen::AtOnce { .. } => observed.delays.push(0),
                Reopen::After(delay) => observed.delays.push(milliseconds(delay)),
            }
        } else {
            break;
        }
    }
    Ok(())
}

fn summarize(requests: &[Captured], observed: &mut Observed) -> TestResult {
    for request in requests {
        if request.method == "POST" && request.path == "/worker/events/delivery" {
            let body: Value = serde_json::from_str(&request.body)?;
            for update in list(&body, "updates")? {
                observed.acks.push((text(update, "event_id")?, text(update, "status")?));
            }
        } else if request.method == "POST" && request.path == "/worker/events" {
            let body: Value = serde_json::from_str(&request.body)?;
            for event in list(&body, "events")? {
                let payload = &event["payload"];
                if text(payload, "type")? == "control_response" {
                    observed
                        .refused_requests
                        .push(text(&payload["response"], "request_id")?);
                }
            }
        } else if request.method == "POST" && request.path == "/worker/heartbeat" {
            observed.probes += 1;
        }
    }
    Ok(())
}

async fn play(case: &Value) -> Result<Observed, Box<dyn std::error::Error>> {
    let (base, log) = serve(case).await?;
    let (attached, hosted) = attach(case, &base).await?;
    let mut session = attached.map_err(|error| error.to_string())?;
    let mut observed = Observed::default();
    read_stream(case, &mut session, &mut observed).await?;
    session.pump().await;
    summarize(&requests_of(&log), &mut observed)?;
    observed.closes = taken(&hosted.closes);
    observed.inbound = taken(&hosted.inbound);
    observed.permission_responses = taken(&hosted.permissions);
    observed.last_sequence = session.sequence_num();
    Ok(observed)
}

fn recorded_gets(case: &Value) -> Result<Vec<StreamRequest>, Box<dyn std::error::Error>> {
    let mut gets = Vec::new();
    for request in list(case, "requests")? {
        let path = text(request, "path")?;
        if text(request, "method")? == "GET" && path.starts_with(STREAM_PATH) {
            let query = path
                .strip_prefix(STREAM_PATH)
                .and_then(|rest| rest.strip_prefix('?'))
                .map(str::to_owned);
            let last_event_id = list(request, "headers")?.iter().find_map(|header| {
                let pair = header.as_array()?;
                (pair.first()?.as_str()?.eq_ignore_ascii_case("last-event-id"))
                    .then(|| pair.get(1)?.as_str().map(str::to_owned))?
            });
            gets.push(StreamRequest { query, last_event_id });
        }
    }
    Ok(gets)
}

fn recorded(case: &Value) -> Result<Observed, Box<dyn std::error::Error>> {
    let mut observed = Observed {
        gets: recorded_gets(case)?,
        ..Observed::default()
    };
    let wire: Vec<Captured> = list(case, "requests")?
        .iter()
        .map(|request| {
            Ok(Captured {
                method: text(request, "method")?,
                path: text(request, "path")?,
                headers: Vec::new(),
                body: text(request, "body")?,
            })
        })
        .collect::<Result<_, String>>()?;
    summarize(&wire, &mut observed)?;
    observed.delays = case
        .get("reconnect_base_ms")
        .and_then(Value::as_array)
        .map(|delays| delays.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    observed.closes = list(case, "closes")?
        .iter()
        .filter_map(Value::as_u64)
        .map(u16::try_from)
        .collect::<Result<_, _>>()?;
    observed.inbound = list(case, "inbound")?.clone();
    observed.permission_responses = list(case, "permission_responses")?.clone();
    observed.last_sequence = i64::try_from(number(case, "last_sequence")?)?;
    Ok(observed)
}

fn writes(case: &Value) -> bool {
    list(case, "steps")
        .map(|steps| steps.iter().any(|step| field(step, "op") != "close"))
        .unwrap_or(false)
}

fn goldens() -> Result<Value, serde_json::Error> {
    serde_json::from_str(GOLDENS)
}

#[tokio::test]
async fn every_recorded_stream_case_is_reproduced_through_the_session() -> TestResult {
    let goldens = goldens()?;
    let cases = list(&goldens, "stream")?;
    assert!(cases.len() >= 51, "only {} stream cases", cases.len());
    let mut replayed = 0;
    for case in cases.iter().filter(|case| !writes(case)) {
        let name = text(case, "name")?;
        let mut expected = recorded(case)?;
        // The probes are the beats the server saw, which the recorder counts the same way.
        expected.probes = number(case, "beats")?;
        assert_eq!(expected, play(case).await?, "{name}");
        replayed += 1;
    }
    assert!(replayed >= 49, "only {replayed} stream cases were replayed");
    Ok(())
}

#[tokio::test]
async fn the_recorded_attach_cases_are_reproduced() -> TestResult {
    let goldens = goldens()?;
    let cases = list(&goldens, "attach")?;
    assert!(cases.len() >= 10, "only {} attach cases", cases.len());
    let mut replayed = 0;
    for case in cases.iter().filter(|case| !writes(case)) {
        let name = text(case, "name")?;
        let (base, log) = serve(case).await?;
        let (attached, hosted) = attach(case, &base).await?;
        let rejection = field(case, "rejection");
        let mut session = match attached {
            Ok(session) => session,
            Err(error) => {
                assert_eq!(
                    Some(error.to_string()),
                    rejection["message"].as_str().map(str::to_owned),
                    "{name}"
                );
                assert!(requests_of(&log).is_empty(), "{name} sent requests");
                replayed += 1;
                continue;
            }
        };
        assert!(rejection.is_null(), "{name} should have been rejected");
        let mut observed = Observed::default();
        if case.get("stream").is_some() {
            read_stream(case, &mut session, &mut observed).await?;
        }
        for step in list(case, "steps")? {
            session.close().await;
            if field(step, "again").as_bool() == Some(true) {
                session.close().await;
            }
        }
        session.pump().await;
        let wanted: Vec<&Value> = list(case, "requests")?
            .iter()
            .filter(|request| {
                !field(request, "path")
                    .as_str()
                    .is_some_and(|path| path.starts_with(STREAM_PATH))
            })
            .collect();
        let sent: Vec<Captured> = requests_of(&log);
        assert_eq!(wanted.len(), sent.len(), "{name} request count");
        for (expected, actual) in wanted.into_iter().zip(&sent) {
            assert_request(&name, expected, actual);
        }
        let streams: Vec<StreamRequest> = observed.gets;
        let expected_gets = recorded_gets(case)?;
        assert_eq!(expected_gets, streams, "{name} stream requests");
        assert_eq!(
            list(case, "closes")?
                .iter()
                .filter_map(Value::as_u64)
                .collect::<Vec<_>>(),
            taken(&hosted.closes).into_iter().map(u64::from).collect::<Vec<_>>(),
            "{name} closes"
        );
        assert_eq!(
            field(case, "last_sequence").as_u64().unwrap_or(0),
            u64::try_from(session.sequence_num()).unwrap_or(0),
            "{name} sequence"
        );
        replayed += 1;
    }
    assert!(replayed >= 6, "only {replayed} attach cases were replayed");
    Ok(())
}

#[tokio::test]
async fn a_worker_refused_for_good_closes_the_session_with_4091() -> TestResult {
    let case = json!({"epoch_option": 6, "interval_ms": 600000, "puts": [{"status": 400, "headers": [], "body": ""}]});
    let (base, _log) = serve(&case).await?;
    let (attached, hosted) = attach(&case, &base).await?;
    let session = attached.map_err(|error| error.to_string())?;
    assert_eq!(vec![CLOSE_INIT_FAILED], taken(&hosted.closes));
    assert!(!session.is_connected());
    assert!(session.stream_request().is_none());
    Ok(())
}

#[tokio::test]
async fn a_conflict_while_starting_closes_the_session_with_4090() -> TestResult {
    let case = json!({
        "epoch_option": 6,
        "interval_ms": 600000,
        "puts": [{"status": 409, "headers": [["x-ccr-conflict-reason", "superseded_by_worker"]], "body": ""}],
    });
    let (base, log) = serve(&case).await?;
    let (attached, hosted) = attach(&case, &base).await?;
    let mut session = attached.map_err(|error| error.to_string())?;
    assert_eq!(vec![4090], taken(&hosted.closes));
    assert!(!session.is_connected());
    session.close().await;
    assert_eq!(
        1,
        requests_of(&log)
            .iter()
            .filter(|request| request.method == "PUT")
            .count()
    );
    Ok(())
}

#[tokio::test]
async fn registering_when_no_epoch_is_given_uses_the_epoch_the_service_issued() -> TestResult {
    let case =
        json!({"interval_ms": 600000, "register": {"status": 200, "headers": [], "body": "{\"worker_epoch\":9}"}});
    let (base, log) = serve(&case).await?;
    let (attached, _hosted) = attach(&case, &base).await?;
    let session = attached.map_err(|error| error.to_string())?;
    assert_eq!(9, session.epoch());
    assert!(session.is_connected());
    assert_eq!("cse_abc", session.session_id());
    let routes: Vec<String> = requests_of(&log)
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect();
    assert_eq!(
        vec!["POST /worker/register".to_owned(), "PUT /worker".to_owned()],
        routes
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_registration_is_an_attach_error() -> TestResult {
    let case = json!({"interval_ms": 600000, "register": {"status": 500, "headers": [], "body": ""}});
    let (base, _log) = serve(&case).await?;
    let (attached, _hosted) = attach(&case, &base).await?;
    assert!(matches!(attached, Err(AttachError::Register(_))));
    Ok(())
}

#[tokio::test]
async fn an_upload_only_session_never_asks_for_the_stream() -> TestResult {
    let case = json!({"epoch_option": 6, "interval_ms": 600000});
    let (base, _log) = serve(&case).await?;
    let (attached, _hosted) = attach(&case, &base).await?;
    let session = attached.map_err(|error| error.to_string())?;
    assert!(session.stream_request().is_none());
    assert_eq!(0, session.sequence_num());
    Ok(())
}

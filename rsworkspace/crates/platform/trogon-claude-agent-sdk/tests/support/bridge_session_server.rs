//! The local server and the session plumbing the bridge session tests share: a real TCP server that answers the
//! worker endpoints of a recorded case, a request log, and the options the recorded cases attach with.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use trogon_claude_agent_sdk::{
    AttachBridgeSessionOptions, AttachCallbacks, AttachError, BridgeSession, EnvironmentLookup, WorkerPacing,
    attach_bridge_session,
};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type Session = BridgeSession<reqwest::Client, Recorded>;
pub type Shared<T> = Arc<Mutex<Vec<T>>>;

pub const SESSION_PATH: &str = "/v1/code/sessions/cse_abc";
pub const STREAM_PATH: &str = "/worker/events/stream";
pub const SKIPPED_HEADERS: [&str; 5] = [
    "accept-encoding",
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
];

#[derive(Clone, Debug)]
pub struct Captured {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

pub type Log = Arc<Mutex<Vec<Captured>>>;

/// Records the pauses the worker asks for instead of waiting them out.
#[derive(Clone)]
pub struct Recorded;

impl WorkerPacing for Recorded {
    fn jitter(&self) -> f64 {
        0.0
    }

    async fn pause(&self, _delay: Duration) {}
}

pub fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

pub fn text(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

pub fn number(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

pub fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

pub fn chunk_bytes(chunk: &Value) -> Result<Vec<u8>, String> {
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

pub fn milliseconds(delay: Duration) -> u64 {
    (delay.as_secs_f64() * 1000.0).round() as u64
}

pub async fn read_request(stream: &mut TcpStream) -> Option<Captured> {
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

/// Writes `response`, with `{base}` in its header values replaced by the server origin as the recorder did.
pub async fn write_response(stream: &mut TcpStream, response: &Value, base: &str) -> TestResult {
    let status = field(response, "status").as_u64().unwrap_or(200);
    let body = field(response, "body").as_str().unwrap_or("");
    let mut head = format!("HTTP/1.1 {status} Status\r\n");
    for pair in field(response, "headers").as_array().into_iter().flatten() {
        if let (Some(name), Some(value)) = (pair[0].as_str(), pair[1].as_str()) {
            head.push_str(&format!("{name}: {}\r\n", value.replace("{base}", base)));
        }
    }
    head.push_str(&format!("content-length: {}\r\nconnection: close\r\n\r\n", body.len()));
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

pub fn queue(case: &Value, key: &str) -> Vec<Value> {
    field(case, key).as_array().cloned().unwrap_or_default()
}

pub fn next_response(queue: &[Value], taken: &mut usize) -> Value {
    let at = (*taken).min(queue.len().saturating_sub(1));
    *taken += 1;
    queue
        .get(at)
        .cloned()
        .unwrap_or_else(|| json!({"status": 200, "headers": [], "body": "{}"}))
}

/// Serves the worker endpoints of a case: each queue answers in order and then repeats its last answer. The read
/// stream is not served, the tests feed it to the session themselves. The stored state read (`GET /worker`) is
/// answered with the case's `record`, or an empty record, and left out of the log, as the recorder did.
pub async fn serve(case: &Value) -> Result<(String, Log), std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let log: Log = Arc::default();
    let shared = log.clone();
    let register = case
        .get("register")
        .filter(|response| !response.is_null())
        .cloned()
        .unwrap_or_else(|| json!({"status": 200, "headers": [], "body": "{\"worker_epoch\":5}"}));
    let record = case
        .get("record")
        .filter(|response| !response.is_null())
        .cloned()
        .unwrap_or_else(|| json!({"status": 200, "headers": [], "body": "{}"}));
    let (puts, landings, beats, events, deliveries) = (
        queue(case, "puts"),
        queue(case, "landing"),
        queue(case, "beat"),
        queue(case, "events"),
        queue(case, "deliveries"),
    );
    let origin = base.clone();
    tokio::spawn(async move {
        let (mut put_count, mut landing_count, mut beat_count, mut event_count, mut delivery_count) = (0, 0, 0, 0, 0);
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
                "GET /worker" => {
                    let _ = write_response(&mut stream, &record, &origin).await;
                    continue;
                }
                "POST /worker/register" => register.clone(),
                "PUT /worker" => next_response(&puts, &mut put_count),
                // Where a redirect may point, answered from the case's `landing` queue as the recorder did.
                "PUT /worker/landing" => next_response(&landings, &mut landing_count),
                "POST /worker/heartbeat" => next_response(&beats, &mut beat_count),
                "POST /worker/events" => next_response(&events, &mut event_count),
                "POST /worker/events/delivery" => next_response(&deliveries, &mut delivery_count),
                _ => json!({"status": 404, "headers": [], "body": ""}),
            };
            if let Ok(mut captured) = shared.lock() {
                captured.push(request);
            }
            let _ = write_response(&mut stream, &response, &origin).await;
        }
    });
    Ok((base, log))
}

pub fn http() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(21))
        .referer(false)
        .build()
}

pub fn sorted_headers(headers: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut kept: Vec<(String, String)> = headers
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .filter(|(name, _)| !SKIPPED_HEADERS.contains(&name.as_str()))
        .collect();
    kept.sort();
    kept
}

pub fn assert_request(name: &str, expected: &Value, actual: &Captured) {
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

pub struct Hosted {
    pub closes: Shared<u16>,
    pub inbound: Shared<Value>,
    pub permissions: Shared<Value>,
}

pub fn pushed<T>(list: &Shared<T>, value: T) {
    if let Ok(mut list) = list.lock() {
        list.push(value);
    }
}

pub fn taken<T: Clone>(list: &Shared<T>) -> Vec<T> {
    list.lock().map(|list| list.clone()).unwrap_or_default()
}

pub fn options(case: &Value, base: &str) -> Result<(AttachBridgeSessionOptions, Hosted), Box<dyn std::error::Error>> {
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
    options.caller_driven = true;
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

pub async fn attach(
    case: &Value,
    base: &str,
) -> Result<(Result<Session, AttachError>, Hosted), Box<dyn std::error::Error>> {
    let (options, hosted) = options(case, base)?;
    Ok((attach_bridge_session(http()?, Recorded, options).await, hosted))
}

pub fn requests_of(log: &Log) -> Vec<Captured> {
    log.lock().map(|log| log.clone()).unwrap_or_default()
}

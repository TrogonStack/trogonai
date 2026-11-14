//! Replays what the upstream `bridge.mjs` did against a local server for each recorded case and checks that the Rust
//! client sends the same request and reaches the same result
//! (`.config/mise/lib/claude-sdk-freshness/bridge_credentials_goldens.mjs` writes the goldens).

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use trogon_claude_agent_sdk::adapters::{CurrentDirectory, EnvironmentVariable};
use trogon_claude_agent_sdk::{
    BranchDropLogDedup, BridgeClient, CodeSessionGitContext, CreateCodeSessionOptions, CreateSessionOutcome,
    CredentialsFailure, DefaultGitProbe, DeviceRefusal, FetchRemoteCredentialsOptions, RemoteCredentialsOutcome,
    SensitiveString,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/bridge_credentials/goldens.json");
const SKIPPED_HEADERS: [&str; 5] = [
    "accept-encoding",
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
];

struct Environment {
    entrypoint: Option<String>,
}

impl EnvironmentVariable for Environment {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        (key == "CLAUDE_CODE_ENTRYPOINT")
            .then(|| self.entrypoint.clone().map(OsString::from))
            .flatten()
    }
}

impl CurrentDirectory for Environment {
    type Error = std::io::Error;

    fn current_dir(&self) -> Result<PathBuf, Self::Error> {
        Ok(PathBuf::from("<cwd>"))
    }
}

#[derive(Clone, Debug)]
struct Captured {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

type Log = Arc<Mutex<Vec<Captured>>>;

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

async fn write_response(stream: &mut TcpStream, response: &Value, extra: &[(String, String)]) -> TestResult {
    let status = field(response, "status").as_u64().unwrap_or(200);
    let body = field(response, "body").as_str().unwrap_or("");
    let mut head = format!("HTTP/1.1 {status} Status\r\n");
    let mut has_length = false;
    for pair in field(response, "headers").as_array().into_iter().flatten() {
        if let (Some(name), Some(value)) = (pair[0].as_str(), pair[1].as_str()) {
            has_length |= name.eq_ignore_ascii_case("content-length");
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
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

/// Serves one recorded case: the first request gets the recorded response (or a redirect, or silence) and any later
/// request gets the recorded final response.
async fn serve(response: Value, final_response: Value) -> Result<(String, Log), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let log: Log = Arc::default();
    let shared = log.clone();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((mut stream, _)) = listener.accept().await {
            let Some(request) = read_request(&mut stream).await else {
                continue;
            };
            let first = {
                let Ok(mut captured) = shared.lock() else { return };
                captured.push(request.clone());
                captured.len() == 1
            };
            if first && response == "hang" {
                held.push(stream);
                continue;
            }
            let _ = if !first {
                write_response(&mut stream, &final_response, &[]).await
            } else if let Some(code) = field(&response, "redirect").as_u64() {
                let target = if request.path.ends_with("/bridge") {
                    "/final/bridge"
                } else {
                    "/final/sessions"
                };
                write_response(
                    &mut stream,
                    &json!({"status": code, "headers": [], "body": ""}),
                    &[("location".to_owned(), target.to_owned())],
                )
                .await
            } else {
                write_response(&mut stream, &response, &[]).await
            };
        }
    });
    Ok((base, log))
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
    let keep = |name: &str| !SKIPPED_HEADERS.contains(&name.to_ascii_lowercase().as_str());
    let want: Vec<(String, String)> = field(expected, "headers")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| Some((pair[0].as_str()?.to_ascii_lowercase(), pair[1].as_str()?.to_owned())))
        .filter(|(header, _)| keep(header))
        .collect();
    let got: Vec<(String, String)> = actual
        .headers
        .iter()
        .map(|(header, value)| (header.to_ascii_lowercase(), value.clone()))
        .filter(|(header, _)| keep(header))
        .collect();
    // The client rebuilds the headers when it follows a redirect, so only the first request pins their order.
    let (mut want, mut got) = (want, got);
    if name.ends_with(" final") {
        want.sort();
        got.sort();
    }
    assert_eq!(want, got, "{name} headers");
}

fn number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9.0e15 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a whole number well inside the i64 range"
        )]
        return Value::from(value as i64);
    }
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn refusal_json(refusal: &DeviceRefusal) -> Value {
    match refusal {
        DeviceRefusal::TokenRefused => json!({"kind": "token_refused"}),
        DeviceRefusal::SignatureWanted { sub_reason } => json!({"kind": "signature_wanted", "subReason": sub_reason}),
        DeviceRefusal::Unrecognized { sub_reason } => json!({"kind": "unrecognized", "subReason": sub_reason}),
    }
}

fn credentials_json(outcome: &RemoteCredentialsOutcome) -> Value {
    match outcome {
        RemoteCredentialsOutcome::Credentials(credentials) => {
            let mut map = Map::new();
            map.insert("worker_jwt".into(), json!(credentials.worker_jwt.expose()));
            map.insert("api_base_url".into(), json!(credentials.api_base_url));
            map.insert("expires_in".into(), number(credentials.expires_in));
            map.insert("worker_epoch".into(), json!(credentials.worker_epoch));
            if let Some(config) = &credentials.mcp_config {
                map.insert("mcp_config".into(), json!({"content": config.content}));
            }
            Value::Object(map)
        }
        RemoteCredentialsOutcome::Failure(failure) => {
            let mut map = Map::new();
            map.insert("terminal".into(), json!(failure.terminal()));
            map.insert("reason".into(), json!(failure.reason()));
            if let Some(status) = failure.status() {
                map.insert("status".into(), json!(status));
            }
            if let Some(source) = failure.source() {
                map.insert("source".into(), json!(source.as_str()));
            }
            if let CredentialsFailure::UntrustedDevice { refusal } = failure {
                map.insert("refusal".into(), refusal_json(refusal));
            }
            Value::Object(map)
        }
        RemoteCredentialsOutcome::Rejected(rejection) => {
            json!({"terminal": rejection.terminal(), "reason": rejection.reason()})
        }
        RemoteCredentialsOutcome::Unavailable => Value::Null,
    }
}

fn create_json(outcome: &CreateSessionOutcome) -> Value {
    match outcome {
        CreateSessionOutcome::Created(id) => json!(id),
        CreateSessionOutcome::Failure(failure) => {
            let mut map = Map::new();
            map.insert("terminal".into(), json!(failure.terminal()));
            map.insert("reason".into(), json!(failure.reason()));
            map.insert("status".into(), json!(failure.status()));
            if let Some(detail) = failure.detail() {
                map.insert("detail".into(), json!(detail));
            }
            Value::Object(map)
        }
        CreateSessionOutcome::Rejected(rejection) => {
            json!({"terminal": rejection.terminal(), "reason": rejection.reason()})
        }
        CreateSessionOutcome::Unavailable => Value::Null,
    }
}

async fn base_for(case: &Value, success_body: String) -> Result<(String, Log), Box<dyn std::error::Error>> {
    let response = field(case, "response").clone();
    if response == "refused" {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        return Ok((base, Log::default()));
    }
    let final_response = match field(&response, "final") {
        Value::Null => json!({"status": 200, "headers": [], "body": success_body}),
        recorded => recorded.clone(),
    };
    serve(response, final_response).await
}

fn check_captured(name: &str, case: &Value, log: &Log) -> TestResult {
    let expected = field(case, "request");
    let captured = log.lock().map_err(|_| "poisoned log")?;
    if expected.is_null() {
        assert!(captured.is_empty(), "{name} sent a request");
        return Ok(());
    }
    assert_request(name, expected, captured.first().ok_or("no request arrived")?);
    if let Some(last) = field(expected, "final").as_object() {
        let last = Value::Object(last.clone());
        assert_request(
            &format!("{name} final"),
            &last,
            captured.last().ok_or("no request arrived")?,
        );
    }
    Ok(())
}

fn http() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(21))
        .referer(false)
        .build()
}

fn timeout(case: &Value) -> Duration {
    Duration::from_millis(field(&case["args"], "timeoutMs").as_u64().unwrap_or(5000))
}

#[tokio::test]
async fn fetch_remote_credentials_matches_the_upstream_client() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let cases = goldens["credentials"].as_array().ok_or("no credentials cases")?;
    assert!(cases.len() > 100, "the corpus shrank to {} cases", cases.len());
    for case in cases {
        let name = text(&case["name"]).ok_or("unnamed case")?;
        let args = &case["args"];
        let ok_body = serde_json::to_string(&case["result"])?;
        let (base, log) = base_for(case, ok_body).await?;
        let client = BridgeClient::new(
            http()?,
            Environment {
                entrypoint: text(field(args, "entrypoint")),
            },
            DefaultGitProbe,
        );
        let options = FetchRemoteCredentialsOptions {
            session_id: text(field(args, "sessionId")).unwrap_or_else(|| "cse_abc123".to_owned()),
            base_url: base,
            access_token: SensitiveString::from("access-token"),
            timeout: Some(timeout(case)),
            trusted_device_token: text(field(args, "trustedDeviceToken")).map(SensitiveString::from),
        };
        let outcome = client.fetch_remote_credentials(&options).await;
        assert_eq!(credentials_json(&outcome), case["result"], "{name} result");
        check_captured(&name, case, &log)?;
    }
    Ok(())
}

#[tokio::test]
async fn create_code_session_matches_the_upstream_client() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let cases = goldens["create"].as_array().ok_or("no create cases")?;
    assert!(cases.len() > 100, "the corpus shrank to {} cases", cases.len());
    for case in cases {
        let name = text(&case["name"]).ok_or("unnamed case")?;
        let args = &case["args"];
        let ok_body = serde_json::to_string(&json!({"session": {"id": case["result"]}}))?;
        let (base, log) = base_for(case, ok_body).await?;
        let client = BridgeClient::new(http()?, Environment { entrypoint: None }, DefaultGitProbe);
        let git_context = field(args, "gitContext")
            .as_object()
            .and_then(|context| context.get("gitRepoUrl"))
            .map(|url| CodeSessionGitContext {
                git_repo_url: text(url).unwrap_or_default(),
                branch: text(field(field(args, "gitContext"), "branch")),
                default_branch: text(field(field(args, "gitContext"), "defaultBranch")),
            });
        let options = CreateCodeSessionOptions {
            base_url: base,
            access_token: SensitiveString::from("access-token"),
            title: text(field(args, "title")).unwrap_or_else(|| "My session".to_owned()),
            timeout: Some(timeout(case)),
            tags: field(args, "tags")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(text)
                .collect(),
            git_context,
            cwd: text(field(args, "cwd")),
            model: text(field(args, "model")),
            session_grouping_id: text(field(args, "sessionGroupingId")),
        };
        let mut dedup = field(args, "dedup").as_object().map(|state| {
            text(field(&Value::Object(state.clone()), "lastKey"))
                .map_or_else(BranchDropLogDedup::new, BranchDropLogDedup::with_last_key)
        });
        let outcome = client.create_code_session(&options, dedup.as_mut()).await?;
        assert_eq!(create_json(&outcome), case["result"], "{name} result");
        check_captured(&name, case, &log)?;
        if let Some(dedup) = &dedup {
            assert_eq!(
                dedup.last_key(),
                text(field(case, "dedupAfter")).as_deref(),
                "{name} dedup"
            );
        }
    }
    Ok(())
}

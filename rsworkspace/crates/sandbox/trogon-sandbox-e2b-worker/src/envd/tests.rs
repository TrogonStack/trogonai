use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use trogon_sandbox_contract::{FileRecursion, FileWatchEventKind, FileWatchEventKind as K};
use url::Url;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::constants::{END_STREAM_FLAG, MAX_FILESYSTEM_REPLY_BYTES, MAX_FRAME_BYTES};
use super::frame::encode_frame;
use super::frame_reader::FrameReader;
use super::process_stream::parse_process_stream;
use super::watcher_batch::WatcherBatch;
use super::*;
use crate::api::{ApiError, E2bConfig};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const ELL_SHA256: &str = "baea96500997ff5cd6cfd26592a978d6b73d480b4ad33d002499cf0041ac9996";

/// A Connect stream of `events` closed by an end-stream frame carrying `end_stream`.
fn test_stream(events: &[Value], end_stream: Option<Value>) -> Result<Vec<u8>, ApiError> {
    let mut out = Vec::new();
    for event in events {
        out.extend(encode_frame(event)?);
    }
    let end = end_stream.unwrap_or_else(|| json!({}));
    let body = serde_json::to_vec(&end).map_err(|error| ApiError::Transport(error.to_string()))?;
    out.push(END_STREAM_FLAG);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn b64(text: &str) -> String {
    STANDARD.encode(text)
}

fn envd_for(server: &MockServer) -> Result<(Envd, EnvdTarget), Box<dyn std::error::Error>> {
    let config = E2bConfig::new(Url::parse("https://api.e2b.app")?, "e2b.app", "k")?
        .with_envd_url(Url::parse(&server.uri())?)?;
    let target = EnvdTarget::for_sandbox(&config, "sbx1", None, Some("tok".to_owned()))?;
    Ok((Envd::new(Duration::from_secs(5))?, target))
}

fn request(argv: &[&str]) -> ProcessRequest {
    ProcessRequest {
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        env: BTreeMap::from([("A".to_owned(), "1".to_owned())]),
        cwd: Some("/work".to_owned()),
        user: "user".to_owned(),
        timeout: Duration::from_secs(30),
    }
}

fn pty_request() -> PtyRequest {
    PtyRequest {
        tag: "t1".to_owned(),
        cmd: "/bin/bash".to_owned(),
        args: vec!["-l".to_owned()],
        env: BTreeMap::from([("TERM".to_owned(), "xterm".to_owned())]),
        user: "user".to_owned(),
        cols: 80,
        rows: 24,
    }
}

fn config() -> Result<E2bConfig, Box<dyn std::error::Error>> {
    Ok(E2bConfig::new(Url::parse("https://api.e2b.app")?, "e2b.app", "k")?)
}

async fn serve_files(server: &MockServer, file: &str, reply: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(query_param("path", file))
        .respond_with(reply)
        .mount(server)
        .await;
}

async fn serve_filesystem(server: &MockServer, rpc: &str, reply: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(format!("/filesystem.Filesystem/{rpc}")))
        .respond_with(reply)
        .mount(server)
        .await;
}

fn body_frame(request: &Request) -> Option<Value> {
    serde_json::from_slice(request.body.get(5..)?).ok()
}

#[test]
fn watcher_events_map_named_and_numeric_types_and_skip_the_rest() {
    let batch = WatcherBatch::from_reply(&json!({"events": [
        {"name": "a", "type": "EVENT_TYPE_CREATE", "entry": {"type": "FILE_TYPE_DIRECTORY"}},
        {"name": "b", "type": 2},
        {"name": "c", "type": "EVENT_TYPE_CHMOD"},
        {"name": "d", "type": 3},
        {"name": "e", "type": "EVENT_TYPE_RENAME"},
        {"name": "f", "type": "EVENT_TYPE_UNSPECIFIED"},
        {"name": "g"},
        {"name": "h", "type": 1, "entry": {"type": 2}},
        {"name": "i", "type": 99},
        {"name": "j", "type": -1},
        {"name": "k", "type": true},
        {"type": "EVENT_TYPE_WRITE"},
    ]}));
    let shape: Vec<_> = batch
        .events
        .iter()
        .map(|event| (event.name.as_str(), event.kind, event.is_dir))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("a", K::Created, true),
            ("b", K::Modified, false),
            ("c", K::Modified, false),
            ("d", K::Deleted, false),
            ("e", K::Deleted, false),
            ("h", K::Created, true),
            ("", K::Modified, false),
        ]
    );
    assert_eq!(batch.dropped, 0);
    assert!(WatcherBatch::from_reply(&Value::Null).events.is_empty());
}

#[test]
fn an_oversized_watcher_batch_keeps_the_ceiling_and_counts_the_rest() {
    let events: Vec<Value> = (0..MAX_WATCHER_EVENTS_PER_FETCH + 3)
        .map(|index| json!({"name": format!("f{index}"), "type": "EVENT_TYPE_WRITE"}))
        .collect();
    let batch = WatcherBatch::from_reply(&json!({ "events": events }));
    assert_eq!(batch.events.len(), MAX_WATCHER_EVENTS_PER_FETCH);
    assert_eq!(batch.dropped, 3);
}

#[test]
fn the_default_envd_address_uses_the_sandbox_domain() -> TestResult {
    let config = config()?;
    let target = EnvdTarget::for_sandbox(&config, "sbx1", None, None)?;
    assert_eq!(target.base.as_str(), "https://49983-sbx1.e2b.app/");
    let target = EnvdTarget::for_sandbox(&config, "sbx1", Some("corp.example"), None)?;
    assert_eq!(target.base.as_str(), "https://49983-sbx1.corp.example/");
    Ok(())
}

#[test]
fn a_sandbox_id_that_cannot_form_an_address_is_refused() -> TestResult {
    let result = EnvdTarget::for_sandbox(&config()?, "a b", None, None);
    assert!(matches!(result, Err(EnvdTargetError::InvalidAddress(_))));
    Ok(())
}

#[test]
fn target_debug_hides_the_token() -> TestResult {
    let target = EnvdTarget::for_sandbox(&config()?, "s", None, Some("very-secret".into()))?;
    let shown = format!("{target:?}");
    assert!(!shown.contains("very-secret"));
    assert!(shown.contains("<redacted>"));
    Ok(())
}

#[test]
fn clones_share_a_token_that_can_be_replaced() -> TestResult {
    let mut target = EnvdTarget::for_sandbox(&config()?, "s", None, Some("one".into()))?;
    let clone = target.clone();
    assert!(target.same_instance(&clone));
    target.replace_token(Some("two".into()));
    assert_eq!(clone.access_header().ok_or("header")?.as_bytes(), b"two");
    let other = EnvdTarget::for_sandbox(&config()?, "s", None, None)?;
    assert!(!target.same_instance(&other));
    assert!(other.access_header().is_none());
    target.replace_token(Some("bad\ntoken".into()));
    assert!(target.access_header().is_none());
    Ok(())
}

#[test]
fn frames_split_across_chunks_are_reassembled() -> TestResult {
    let event = json!({"event": {"start": {"pid": 7}}});
    let whole = encode_frame(&event)?;
    let mut reader = FrameReader::default();
    for byte in &whole {
        assert!(reader.next_frame()?.is_none());
        reader.push(&[*byte]);
    }
    let (flags, payload) = reader.next_frame()?.ok_or("frame")?;
    assert_eq!(flags, 0);
    assert_eq!(serde_json::from_slice::<Value>(&payload)?, event);
    assert!(reader.next_frame()?.is_none());
    Ok(())
}

#[test]
fn an_oversized_frame_is_refused_instead_of_buffered() {
    let mut reader = FrameReader::default();
    let mut header = vec![0u8];
    header.extend_from_slice(&((MAX_FRAME_BYTES as u32) + 1).to_be_bytes());
    reader.push(&header);
    assert!(matches!(reader.next_frame(), Err(ApiError::Transport(_))));
}

#[test]
fn selectors_use_the_proto_field_names() -> TestResult {
    assert_eq!(serde_json::to_value(ProcessSelector::Pid(9))?, json!({"pid": 9}));
    assert_eq!(
        serde_json::to_value(ProcessSelector::Tag("t".into()))?,
        json!({"tag": "t"})
    );
    Ok(())
}

#[test]
fn stream_collects_output_and_exit_status() -> TestResult {
    let body = test_stream(
        &[
            json!({"event": {"start": {"pid": 7}}}),
            json!({"event": {"data": {"stdout": b64("hel")}}}),
            json!({"event": {"data": {"stderr": b64("warn")}}}),
            json!({"event": {"data": {"stdout": b64("lo")}}}),
            json!({"event": {"keepalive": {}}}),
            json!({"keepalive": {}}),
            json!({"event": {"end": {"exitCode": 3, "exited": true, "status": "exit status 3"}}}),
        ],
        None,
    )?;
    let result = parse_process_stream(&body)?;
    assert_eq!(result.stdout, b"hello");
    assert_eq!(result.stderr, b"warn");
    assert_eq!(result.exit_code, 3);
    Ok(())
}

#[test]
fn a_clean_exit_may_omit_the_zero_exit_code() -> TestResult {
    let body = test_stream(
        &[json!({"event": {"end": {"exited": true, "status": "exit status 0"}}})],
        None,
    )?;
    assert_eq!(parse_process_stream(&body)?.exit_code, 0);
    Ok(())
}

#[test]
fn a_signalled_process_has_no_clean_exit() -> TestResult {
    let body = test_stream(&[json!({"event": {"end": {"status": "signal: killed"}}})], None)?;
    assert_eq!(parse_process_stream(&body)?.exit_code, -1);
    Ok(())
}

#[test]
fn a_deadline_without_native_end_preserves_uncertainty() -> TestResult {
    for code in ["deadline_exceeded", "canceled"] {
        let body = test_stream(
            &[json!({"event": {"data": {"stdout": b64("partial")}}})],
            Some(json!({"error": {"code": code, "message": "too slow"}})),
        )?;
        assert!(matches!(
            parse_process_stream(&body),
            Err(ApiError::Transport(message)) if message.contains("termination is unconfirmed")
        ));
    }
    Ok(())
}

#[test]
fn a_transport_deadline_after_native_end_preserves_actual_status() -> TestResult {
    let body = test_stream(
        &[json!({"event": {"end": {"exited": true, "exitCode": 137}}})],
        Some(json!({"error": {"code": "deadline_exceeded", "message": "late trailer"}})),
    )?;
    let result = parse_process_stream(&body)?;
    assert_eq!(result.exit_code, 137);
    Ok(())
}

#[test]
fn stream_errors_map_to_api_errors() -> TestResult {
    let denied = test_stream(
        &[],
        Some(json!({"error": {"code": "unauthenticated", "message": "bad token"}})),
    )?;
    assert!(matches!(parse_process_stream(&denied), Err(ApiError::Unauthorized(_))));
    let missing = test_stream(&[], Some(json!({"error": {"code": "not_found", "message": "gone"}})))?;
    assert!(matches!(parse_process_stream(&missing), Err(ApiError::NotFound(_))));
    let bad = test_stream(
        &[],
        Some(json!({"error": {"code": "invalid_argument", "message": "cwd missing"}})),
    )?;
    assert!(matches!(parse_process_stream(&bad), Err(ApiError::Rejected { .. })));
    assert!(matches!(
        parse_process_stream(&test_stream(&[], None)?),
        Err(ApiError::Transport(_))
    ));
    assert!(matches!(parse_process_stream(&[0, 0, 0]), Err(ApiError::Transport(_))));
    assert!(matches!(
        parse_process_stream(&[0, 0, 0, 0, 9, b'{']),
        Err(ApiError::Transport(_))
    ));
    let mut garbled = vec![0, 0, 0, 0, 1];
    garbled.push(b'x');
    assert!(matches!(parse_process_stream(&garbled), Err(ApiError::Transport(_))));
    Ok(())
}

#[tokio::test]
async fn run_posts_a_connect_request_with_the_sandbox_token() -> TestResult {
    let server = MockServer::start().await;
    let body = test_stream(
        &[
            json!({"event": {"data": {"stdout": b64("ok\n")}}}),
            json!({"event": {"end": {"exited": true}}}),
        ],
        None,
    )?;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(header("X-Access-Token", "tok"))
        .and(header("Connect-Protocol-Version", "1"))
        .and(header("Content-Type", "application/connect+json"))
        .and(header("Connect-Timeout-Ms", "30000"))
        .and(header("Authorization", "Basic dXNlcjo="))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .expect(1)
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    let result = envd.run(&target, &request(&["echo", "ok"])).await?;
    assert_eq!(result.stdout, b"ok\n");
    assert_eq!(result.exit_code, 0);
    Ok(())
}

#[tokio::test]
async fn run_sends_argv_env_and_cwd_without_shell_quoting() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(|request: &Request| {
            body_frame(request).is_some_and(|body| {
                body["process"]["cmd"] == "/bin/bash"
                    && body["process"]["args"] == json!(["-l", "-c", "exec \"$@\"", "bash", "echo", "it's $HOME"])
                    && body["process"]["envs"] == json!({"A": "1"})
                    && body["process"]["cwd"] == "/work"
                    && body["stdin"] == false
                    && body.get("pty").is_none()
                    && body.get("tag").is_none()
            })
        })
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(test_stream(&[json!({"event": {"end": {"exited": true}}})], None)?),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    envd.run(&target, &request(&["echo", "it's $HOME"])).await?;
    Ok(())
}

#[tokio::test]
async fn run_omits_the_working_directory_when_there_is_none() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(|request: &Request| body_frame(request).is_some_and(|body| body["process"].get("cwd").is_none()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(test_stream(&[json!({"event": {"end": {"exited": true}}})], None)?),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    let mut process = request(&["true"]);
    process.cwd = None;
    envd.run(&target, &process).await?;
    Ok(())
}

#[tokio::test]
async fn run_maps_http_failures() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(
            ResponseTemplate::new(502)
                .set_body_json(json!({"sandboxId": "sbx1", "message": "The sandbox was not found", "code": 502})),
        )
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    assert!(matches!(
        envd.run(&target, &request(&["true"])).await,
        Err(ApiError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_bad_gateway_that_is_not_a_missing_sandbox_is_unavailable() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(ResponseTemplate::new(502).set_body_json(json!({"message": "upstream down"})))
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    assert!(matches!(
        envd.run(&target, &request(&["true"])).await,
        Err(ApiError::Unavailable { status: 502, .. })
    ));
    Ok(())
}

#[tokio::test]
async fn an_unreachable_envd_is_a_transport_error() -> TestResult {
    let config = config()?.with_envd_url(Url::parse("http://127.0.0.1:1")?)?;
    let target = EnvdTarget::for_sandbox(&config, "sbx1", None, None)?;
    let envd = Envd::new(Duration::from_secs(2))?;
    assert!(matches!(
        envd.run(&target, &request(&["true"])).await,
        Err(ApiError::Transport(_))
    ));
    assert!(matches!(
        envd.read_file(&target, "user", "/a").await,
        Err(ApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_target_without_a_token_sends_no_access_header() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(|request: &Request| !request.headers.contains_key("X-Access-Token"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x"))
        .expect(1)
        .mount(&server)
        .await;
    let config = config()?.with_envd_url(Url::parse(&server.uri())?)?;
    let target = EnvdTarget::for_sandbox(&config, "sbx1", None, None)?;
    let envd = Envd::new(Duration::from_secs(5))?;
    assert_eq!(envd.read_file(&target, "user", "/a").await?, Some(b"x".to_vec()));
    Ok(())
}

#[tokio::test]
async fn pty_start_streams_events_until_the_end() -> TestResult {
    let server = MockServer::start().await;
    let body = test_stream(
        &[
            json!({"event": {"start": {"pid": 42}}}),
            json!({"event": {"keepalive": {}}}),
            json!({}),
            json!({"event": {"data": {"pty": b64("$ ")}}}),
            json!({"event": {"data": {"stdout": b64("ignored")}}}),
            json!({"event": {"end": {"exited": true, "exitCode": 2}}}),
        ],
        None,
    )?;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(header("Keepalive-Ping-Interval", "50"))
        .and(header("Connect-Timeout-Ms", "86400000"))
        .and(|request: &Request| {
            body_frame(request).is_some_and(|body| {
                body["pty"] == json!({"size": {"cols": 80, "rows": 24}})
                    && body["tag"] == "t1"
                    && body["stdin"] == true
                    && body["process"]["cmd"] == "/bin/bash"
                    && body["process"]["args"] == json!(["-l"])
                    && body["process"]["envs"] == json!({"TERM": "xterm"})
            })
        })
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .expect(1)
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    let mut stream = envd.start_pty(&target, &pty_request()).await?;
    assert_eq!(stream.next().await?, Some(PtyEvent::Started(42)));
    assert_eq!(stream.next().await?, Some(PtyEvent::Data(b"$ ".to_vec())));
    assert_eq!(stream.next().await?, Some(PtyEvent::Ended(2)));
    assert_eq!(stream.next().await?, None);
    assert_eq!(stream.next().await?, None);
    Ok(())
}

#[tokio::test]
async fn a_pty_stream_reports_its_error_trailer() -> TestResult {
    let server = MockServer::start().await;
    let body = test_stream(
        &[],
        Some(json!({"error": {"code": "permission_denied", "message": "no"}})),
    )?;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    let mut stream = envd.start_pty(&target, &pty_request()).await?;
    assert!(matches!(stream.next().await, Err(ApiError::Unauthorized(_))));
    assert_eq!(stream.next().await?, None);
    Ok(())
}

#[tokio::test]
async fn a_pty_stream_with_a_clean_trailer_or_a_cut_connection_just_ends() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(test_stream(&[], None)?))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0, 0, 0]))
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    let mut clean = envd.start_pty(&target, &pty_request()).await?;
    assert_eq!(clean.next().await?, None);
    let mut cut = envd.start_pty(&target, &pty_request()).await?;
    assert_eq!(cut.next().await?, None);
    Ok(())
}

#[tokio::test]
async fn a_pty_stream_refuses_a_garbled_frame() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0, 0, 0, 0, 1, b'x']))
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    let mut stream = envd.start_pty(&target, &pty_request()).await?;
    assert!(matches!(stream.next().await, Err(ApiError::Transport(_))));
    Ok(())
}

#[tokio::test]
async fn pty_start_surfaces_http_failures() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"message": "bad token"})))
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    assert!(matches!(
        envd.start_pty(&target, &pty_request()).await,
        Err(ApiError::Unauthorized(_))
    ));
    Ok(())
}

#[tokio::test]
async fn pty_input_resize_and_kill_use_the_unary_rpcs() -> TestResult {
    let server = MockServer::start().await;
    for (rpc, expected) in [
        (
            "SendInput",
            json!({"process": {"tag": "t1"}, "input": {"pty": b64("ls\n")}}),
        ),
        (
            "Update",
            json!({"process": {"pid": 9}, "pty": {"size": {"cols": 100, "rows": 30}}}),
        ),
        ("SendSignal", json!({"process": {"pid": 9}, "signal": "SIGNAL_SIGKILL"})),
    ] {
        Mock::given(method("POST"))
            .and(path(format!("/process.Process/{rpc}")))
            .and(header("Connect-Protocol-Version", "1"))
            .and(header("Content-Type", "application/json"))
            .and(header("X-Access-Token", "tok"))
            .and(body_json(expected))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
    }
    let (envd, target) = envd_for(&server)?;
    envd.send_pty_input(&target, "user", &ProcessSelector::Tag("t1".into()), b"ls\n")
        .await?;
    envd.resize_pty(&target, "user", &ProcessSelector::Pid(9), 100, 30)
        .await?;
    envd.kill_process(&target, "user", &ProcessSelector::Pid(9)).await?;
    Ok(())
}

#[tokio::test]
async fn a_failed_unary_call_is_an_api_error() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/SendSignal"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"code": "not_found", "message": "no process"})))
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    assert!(matches!(
        envd.kill_process(&target, "user", &ProcessSelector::Pid(1)).await,
        Err(ApiError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn filesystem_replies_are_parsed_bounded_and_may_be_empty() -> TestResult {
    let server = MockServer::start().await;
    serve_filesystem(
        &server,
        "Stat",
        ResponseTemplate::new(200).set_body_json(json!({"entry": {}})),
    )
    .await;
    serve_filesystem(&server, "Empty", ResponseTemplate::new(200)).await;
    serve_filesystem(
        &server,
        "Garbled",
        ResponseTemplate::new(200).set_body_string("not json"),
    )
    .await;
    serve_filesystem(
        &server,
        "Huge",
        ResponseTemplate::new(200).set_body_string("x".repeat(MAX_FILESYSTEM_REPLY_BYTES + 1)),
    )
    .await;
    serve_filesystem(&server, "Denied", ResponseTemplate::new(403)).await;
    let (envd, target) = envd_for(&server)?;
    let stat = envd
        .filesystem_unary(&target, "user", "Stat", json!({"path": "/a"}))
        .await?;
    assert_eq!(stat, json!({"entry": {}}));
    assert_eq!(
        envd.filesystem_unary(&target, "user", "Empty", json!({})).await?,
        Value::Null
    );
    assert!(matches!(
        envd.filesystem_unary(&target, "user", "Garbled", json!({})).await,
        Err(ApiError::Transport(_))
    ));
    assert!(matches!(
        envd.filesystem_unary(&target, "user", "Huge", json!({})).await,
        Err(ApiError::Transport(message)) if message.contains("size limit")
    ));
    assert!(matches!(
        envd.filesystem_unary(&target, "user", "Denied", json!({})).await,
        Err(ApiError::Unauthorized(_))
    ));
    Ok(())
}

#[tokio::test]
async fn watchers_are_created_polled_and_removed() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .and(body_json(
            json!({"path": "/work", "recursive": true, "includeEntry": true}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"watcherId": "w1"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/GetWatcherEvents"))
        .and(body_json(json!({"watcherId": "w1"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"events": [{"name": "a.txt", "type": "EVENT_TYPE_CREATE"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/RemoveWatcher"))
        .and(body_json(json!({"watcherId": "w1"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    let id = envd
        .create_watcher(&target, "user", "/work", FileRecursion::Recursive)
        .await?;
    assert_eq!(id, "w1");
    let batch = envd.watcher_events(&target, "user", &id).await?;
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.events[0].name, "a.txt");
    assert_eq!(batch.events[0].kind, FileWatchEventKind::Created);
    envd.remove_watcher(&target, "user", &id).await?;
    Ok(())
}

#[tokio::test]
async fn a_watcher_reply_without_an_id_is_refused() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .and(body_json(
            json!({"path": "/flat", "recursive": false, "includeEntry": true}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"watcher_id": "snake"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"watcherId": ""})))
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    let snake = envd
        .create_watcher(&target, "user", "/flat", FileRecursion::Flat)
        .await?;
    assert_eq!(snake, "snake");
    assert!(matches!(
        envd.create_watcher(&target, "user", "/other", FileRecursion::Recursive)
            .await,
        Err(ApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn read_file_distinguishes_missing_from_failure() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(query_param("path", "/a.txt"))
        .and(query_param("username", "user"))
        .respond_with(ResponseTemplate::new(200).set_body_string("data"))
        .mount(&server)
        .await;
    serve_files(
        &server,
        "/missing",
        ResponseTemplate::new(404).set_body_json(json!({"code": 404, "message": "file not found"})),
    )
    .await;
    serve_files(
        &server,
        "/boom",
        ResponseTemplate::new(500).set_body_json(json!({"code": 500, "message": "disk"})),
    )
    .await;
    let (envd, target) = envd_for(&server)?;
    assert_eq!(envd.read_file(&target, "user", "/a.txt").await?, Some(b"data".to_vec()));
    assert_eq!(envd.read_file(&target, "user", "/missing").await?, None);
    assert!(matches!(
        envd.read_file(&target, "user", "/boom").await,
        Err(ApiError::Unavailable { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn capped_reads_report_whether_anything_was_dropped() -> TestResult {
    let server = MockServer::start().await;
    serve_files(&server, "/f", ResponseTemplate::new(200).set_body_string("0123456789")).await;
    serve_files(&server, "/missing", ResponseTemplate::new(404)).await;
    let (envd, target) = envd_for(&server)?;
    assert_eq!(
        envd.read_file_capped(&target, "user", "/f", 100).await?,
        Some((b"0123456789".to_vec(), false))
    );
    let (bytes, truncated) = envd.read_file_capped(&target, "user", "/f", 4).await?.ok_or("file")?;
    assert!(truncated);
    assert!(bytes.starts_with(b"01"));
    assert!(bytes.ends_with(b"89"));
    assert_eq!(envd.read_file_capped(&target, "user", "/missing", 4).await?, None);
    Ok(())
}

#[tokio::test]
async fn ranged_reads_skip_the_offset_and_stop_at_the_count() -> TestResult {
    let server = MockServer::start().await;
    serve_files(&server, "/f", ResponseTemplate::new(200).set_body_string("hello")).await;
    serve_files(&server, "/missing", ResponseTemplate::new(404)).await;
    let (envd, target) = envd_for(&server)?;
    assert_eq!(
        envd.read_file_range(&target, "user", "/f", 1, 3).await?,
        Some(b"ell".to_vec())
    );
    assert_eq!(
        envd.read_file_range(&target, "user", "/f", 0, 100).await?,
        Some(b"hello".to_vec())
    );
    assert_eq!(
        envd.read_file_range(&target, "user", "/f", 0, 0).await?,
        Some(Vec::new())
    );
    assert_eq!(envd.read_file_range(&target, "user", "/missing", 0, 3).await?, None);
    Ok(())
}

#[tokio::test]
async fn ranged_hashes_cover_exactly_the_requested_bytes() -> TestResult {
    let server = MockServer::start().await;
    serve_files(&server, "/f", ResponseTemplate::new(200).set_body_string("hello")).await;
    serve_files(&server, "/missing", ResponseTemplate::new(404)).await;
    let (envd, target) = envd_for(&server)?;
    let whole = envd.hash_file_range(&target, "user", "/f", 0, 5).await?.ok_or("hash")?;
    assert_eq!((whole.0.as_str(), whole.1), (HELLO_SHA256, 5));
    let middle = envd.hash_file_range(&target, "user", "/f", 1, 3).await?.ok_or("hash")?;
    assert_eq!((middle.0.as_str(), middle.1), (ELL_SHA256, 3));
    let empty = envd.hash_file_range(&target, "user", "/f", 0, 0).await?.ok_or("hash")?;
    assert_eq!((empty.0.as_str(), empty.1), (EMPTY_SHA256, 0));
    let short = envd
        .hash_file_range(&target, "user", "/f", 0, 50)
        .await?
        .ok_or("hash")?;
    assert_eq!((short.0.as_str(), short.1), (HELLO_SHA256, 5));
    assert!(envd.hash_file_range(&target, "user", "/missing", 0, 3).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn write_file_uploads_multipart_to_the_path() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/files"))
        .and(query_param("path", "/work/a.txt"))
        .and(query_param("username", "user"))
        .and(header("X-Access-Token", "tok"))
        .and(|request: &Request| {
            let body = String::from_utf8_lossy(&request.body);
            body.contains("name=\"file\"") && body.contains("filename=\"a.txt\"") && body.contains("hello")
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"name": "a.txt", "path": "/work/a.txt", "type": "file"}])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    envd.write_file(&target, "user", "/work/a.txt", b"hello".to_vec())
        .await?;
    Ok(())
}

#[tokio::test]
async fn write_file_surfaces_rejections() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/files"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"code": 401, "message": "bad token"})))
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    assert!(matches!(
        envd.write_file(&target, "user", "/a", vec![]).await,
        Err(ApiError::Unauthorized(_))
    ));
    Ok(())
}

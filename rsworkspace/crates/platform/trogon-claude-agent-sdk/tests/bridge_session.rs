//! Replays the recorded `stream` and `attach` cases (written by
//! `.config/mise/lib/claude-sdk-freshness/bridge_worker_goldens.mjs` against the real `attachBridgeSession`) through
//! [`BridgeSession`]: the requests it sends to a local server and what it reports to the host must match what the
//! upstream entrypoint did.

#[path = "support/bridge_session_server.rs"]
mod server;

use std::time::Duration;

use serde_json::{Value, json};
use server::*;
use trogon_claude_agent_sdk::{AttachError, CLOSE_INIT_FAILED, Reopen, StreamOpen, StreamRequest};

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");

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
    for step in list(case, "steps")? {
        if let Some(frame) = field(step, "frame").as_object() {
            session.write(frame.clone());
        }
    }
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

/// The stream cases whose only steps are frames the host wrote: they are written before the stream is read.
fn host_writes(case: &Value) -> bool {
    list(case, "steps")
        .map(|steps| steps.iter().all(|step| field(step, "op") == "write"))
        .unwrap_or(true)
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
    for case in cases.iter().filter(|case| host_writes(case)) {
        let name = text(case, "name")?;
        let mut expected = recorded(case)?;
        // The probes are the beats the server saw, which the recorder counts the same way.
        expected.probes = number(case, "beats")?;
        assert_eq!(expected, play(case).await?, "{name}");
        replayed += 1;
    }
    assert!(replayed >= 51, "only {replayed} stream cases were replayed");
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

/// With an epoch nothing is registered, so a base URL that does not parse is reported by the attach itself.
#[tokio::test]
async fn an_invalid_base_url_is_an_attach_error() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    for golden in goldens["invalid_url"].as_array().ok_or("no invalid_url cases")? {
        let case = json!({"epoch_option": 2, "interval_ms": 600000});
        let (attached, _hosted) = attach(&case, &text(golden, "base")?).await?;
        let error = attached.err().ok_or("attached")?;
        assert!(matches!(error, AttachError::InvalidUrl), "{golden}");
        assert_eq!(text(golden, "message")?, error.to_string(), "{golden}");
    }
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

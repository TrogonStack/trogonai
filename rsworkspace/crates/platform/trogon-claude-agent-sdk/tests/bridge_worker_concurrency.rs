//! The worker client lets calls overlap the way the upstream timers and uploads do: a heartbeat goes out while an
//! upload waits for its answer, and events are accepted while a batch is in flight. The orders are the ones the pinned
//! upstream bundle produced (the `concurrency` section of the recorded goldens).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use trogon_claude_agent_sdk::{ClientEvent, NonOrigin403, SensitiveString, TokioPacing, WorkerClient, WorkerConfig};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");

/// The order of requests the upstream bundle showed for the case `mode`.
fn recorded(mode: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let goldens: serde_json::Value = serde_json::from_str(GOLDENS)?;
    let case = goldens["concurrency"]
        .as_array()
        .and_then(|cases| cases.iter().find(|case| case["mode"] == mode))
        .ok_or("no such recorded case")?;
    Ok(serde_json::from_value(case["log"].clone())?)
}

/// What the server saw, in the order it saw it: `<path> in` when a request arrived, `<path> out` when it was answered.
type Log = Arc<Mutex<Vec<String>>>;

fn note(log: &Log, line: String) {
    if let Ok(mut lines) = log.lock() {
        lines.push(line);
    }
}

fn seen(log: &Log) -> Vec<String> {
    log.lock().map(|lines| lines.clone()).unwrap_or_default()
}

async fn request_path(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&bytes);
    let line = head.lines().next()?;
    line.split(' ').nth(1).map(str::to_owned)
}

/// Answers every request `200 {}`, except that `/worker/events` waits until a heartbeat arrived (`beat`) and then for
/// `release`. Each connection is served on its own task, so requests overlap.
async fn server(log: Log, beat: Arc<Notify>, release: Arc<Notify>) -> Result<String, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (log, beat, release) = (Arc::clone(&log), Arc::clone(&beat), Arc::clone(&release));
            tokio::spawn(async move {
                let Some(path) = request_path(&mut stream).await else {
                    return;
                };
                let name = path
                    .rsplit_once("/v1/code/sessions/cse_abc")
                    .map_or(path.as_str(), |(_, rest)| rest);
                let name = name.trim_start_matches("/worker/");
                note(&log, format!("{name} in"));
                if name == "heartbeat" {
                    beat.notify_one();
                }
                if name == "events" {
                    release.notified().await;
                }
                note(&log, format!("{name} out"));
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
                    .await;
            });
        }
    });
    Ok(base)
}

fn worker(base: &str) -> Result<WorkerClient<reqwest::Client, TokioPacing>, Box<dyn std::error::Error>> {
    Ok(WorkerClient::new(
        reqwest::Client::new(),
        TokioPacing,
        WorkerConfig {
            api_base_url: base.to_owned(),
            session_id: "cse_abc".to_owned(),
            token: SensitiveString::from("tok"),
            entrypoint: None,
            heartbeat_interval_ms: None,
            session_access_token: None,
            non_origin_403: NonOrigin403::default(),
        },
    )?)
}

fn event(uuid: &str) -> ClientEvent {
    let mut payload = Map::new();
    payload.insert("type".to_owned(), json!("assistant"));
    payload.insert("uuid".to_owned(), json!(uuid));
    ClientEvent {
        payload,
        historical: false,
        ephemeral: false,
    }
}

#[tokio::test]
async fn heartbeat_runs_while_an_upload_is_in_flight() -> TestResult {
    let (log, beat, release) = (Log::default(), Arc::new(Notify::new()), Arc::new(Notify::new()));
    let base = server(Arc::clone(&log), Arc::clone(&beat), Arc::clone(&release)).await?;
    let worker = worker(&base)?;
    assert_eq!(worker.submit_events(vec![event("e1")]).await, Ok(()));
    let upload = worker.pump_events(None);
    let beating = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let outcome = worker.heartbeat().await;
        beat.notified().await;
        release.notify_one();
        outcome
    };
    let (uploaded, outcome) =
        tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(upload, beating) }).await?;
    assert!(matches!(uploaded, Ok(true)));
    assert!(outcome.is_ok());
    assert_eq!(seen(&log), recorded("heartbeat_during_upload")?);
    Ok(())
}

#[tokio::test]
async fn events_are_accepted_while_a_batch_is_in_flight() -> TestResult {
    let (log, beat, release) = (Log::default(), Arc::new(Notify::new()), Arc::new(Notify::new()));
    let base = server(Arc::clone(&log), Arc::clone(&beat), Arc::clone(&release)).await?;
    let worker = worker(&base)?;
    assert_eq!(worker.submit_events(vec![event("e1")]).await, Ok(()));
    let upload = worker.pump_events(None);
    let writing = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let accepted = worker.submit_events(vec![event("e2")]).await;
        release.notify_one();
        release.notify_one();
        accepted
    };
    let (uploaded, accepted) =
        tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(upload, writing) }).await?;
    assert!(matches!(uploaded, Ok(true)));
    assert!(accepted.is_ok());
    assert_eq!(seen(&log), recorded("events_while_a_batch_is_open")?);
    Ok(())
}

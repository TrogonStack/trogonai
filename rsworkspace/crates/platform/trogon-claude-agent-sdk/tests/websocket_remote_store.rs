#![cfg(feature = "test-support")]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests exercise exact upstream JSON fixtures.")
)]

//! A session over [`WebSocketTransport`] against a fake CLI that speaks stream-json over the socket, with an
//! externally implemented [`SessionStore`] that must receive the mirrored transcript frames.

use std::{
    error::Error as StdError,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::mpsc, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message as Frame};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, Message, Prompt, SessionEntry, SessionKey, SessionProjectKey, SessionStore,
    SessionStoreFuture, SessionStoreOptions, Timeout, WebSocketOptions, WebSocketTransport,
};

type TestResult = Result<(), Box<dyn StdError>>;

/// A store written against the public trait only: it records every append in memory.
#[derive(Default)]
struct RecordingStore {
    appends: Mutex<Vec<(SessionKey, Vec<SessionEntry>)>>,
}

impl SessionStore for RecordingStore {
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            if let Ok(mut appends) = self.appends.lock() {
                appends.push((key.clone(), entries.to_vec()));
            }
            Ok(())
        })
    }

    fn load<'a>(&'a self, _key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

fn line(value: &Value) -> Frame {
    Frame::text(format!("{value}\n"))
}

/// The fake CLI: answers `initialize`, then answers the first user message with a mirrored transcript and a result.
async fn fake_cli(listener: TcpListener, mirror_path: String, seen: mpsc::UnboundedSender<Value>) {
    let Ok((stream, _)) = listener.accept().await else {
        return;
    };
    let Ok(mut socket) = accept_async(stream).await else {
        return;
    };
    while let Some(Ok(Frame::Text(text))) = socket.next().await {
        for document in text.lines().filter(|line| !line.is_empty()) {
            let Ok(value) = serde_json::from_str::<Value>(document) else {
                continue;
            };
            let _ = seen.send(value.clone());
            match value["type"].as_str() {
                Some("control_request") => {
                    let _ = socket
                        .send(line(&json!({
                            "type": "control_response",
                            "response": {"subtype": "success", "request_id": value["request_id"], "response": {}},
                        })))
                        .await;
                }
                Some("user") => {
                    let _ = socket
                        .send(line(&json!({
                            "type": "transcript_mirror", "filePath": mirror_path, "ccr_event": {"seq": 1},
                            "entries": [{"type": "user", "uuid": "entry-1", "message": {"content": "hello"}}],
                        })))
                        .await;
                    let _ = socket
                        .send(line(&json!({
                            "type": "result", "subtype": "success", "is_error": false, "duration_ms": 1,
                            "duration_api_ms": 1, "num_turns": 1, "session_id": "remote-session",
                            "result": "done", "total_cost_usd": 0.0,
                        })))
                        .await;
                }
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn a_remote_session_mirrors_its_transcript_into_an_external_store() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mirror_path = directory
        .path()
        .join("projects/remote-project/remote-session.jsonl")
        .to_string_lossy()
        .into_owned();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}/", listener.local_addr()?);
    let (seen_tx, mut seen) = mpsc::unbounded_channel();
    tokio::spawn(fake_cli(listener, mirror_path, seen_tx));

    let store = Arc::new(RecordingStore::default());
    let mut store_options = SessionStoreOptions::from(store.clone() as Arc<dyn SessionStore>);
    store_options.config_directory = Some(directory.path().to_path_buf());
    store_options.project_key = Some(SessionProjectKey::new("remote-project")?);
    let mut options = ClaudeAgentOptions::default();
    options.limits.control_timeout = Some(Timeout::new(Duration::from_secs(5)));
    options.session_store = Some(store_options);

    let mut client =
        ClaudeSdkClient::connect_with_transport(options, WebSocketTransport::new(WebSocketOptions::new(url))).await?;
    client.send_prompt(Prompt::from("hello")).await?;
    timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Message::Result(_)) = client.next_message().await? {
                return Ok::<(), trogon_claude_agent_sdk::Error>(());
            }
        }
    })
    .await??;

    let appends = store.appends.lock().map_err(|_| "the store lock was poisoned")?.clone();
    assert_eq!(appends.len(), 1);
    assert_eq!(appends[0].0.session_id().to_string(), "remote-session");
    assert_eq!(appends[0].1.len(), 1);

    let initialize = seen.recv().await.ok_or("the CLI saw nothing")?;
    assert_eq!(initialize["request"]["subtype"], "initialize");
    let user = seen.recv().await.ok_or("the CLI saw no prompt")?;
    assert_eq!(user["message"]["content"], "hello");
    client.disconnect().await?;
    Ok(())
}

#![cfg(feature = "test-support")]

//! A caller-supplied `Transport`, Python's `ClaudeSDKClient(transport=...)` and `query(transport=...)`.

use std::{
    error::Error as StdError,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{sync::mpsc, time::timeout};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, Error, InMemorySessionStore, JsonObject, Message, Prompt, SessionEntry,
    SessionId, SessionKey, SessionProjectKey, SessionStore, SessionStoreFuture, SessionStoreOptions, Timeout,
    Transport, query_turn_with_transport, query_with_transport,
};

type TestResult = Result<(), Box<dyn StdError>>;

enum Incoming {
    Object(JsonObject),
    Failure(String),
}

#[derive(Clone, Default)]
struct Calls(Arc<Mutex<Vec<&'static str>>>);

impl Calls {
    fn record(&self, call: &'static str) {
        if let Ok(mut calls) = self.0.lock() {
            calls.push(call);
        }
    }

    fn all(&self) -> Vec<&'static str> {
        self.0.lock().map(|calls| calls.clone()).unwrap_or_default()
    }

    fn count(&self, call: &str) -> usize {
        self.all().iter().filter(|recorded| **recorded == call).count()
    }
}

/// A transport written against the public trait only.
struct ScriptedTransport {
    written: mpsc::UnboundedSender<String>,
    incoming: mpsc::UnboundedReceiver<Incoming>,
    calls: Calls,
    refuse_connect: bool,
    fail_writes: Arc<AtomicBool>,
    ready: bool,
}

impl Transport for ScriptedTransport {
    async fn connect(&mut self) -> Result<(), Error> {
        self.calls.record("connect");
        if self.refuse_connect {
            return Err(Error::Transport("connection refused".into()));
        }
        Ok(())
    }

    async fn write(&mut self, data: &str) -> Result<(), Error> {
        self.calls.record("write");
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(Error::NotReadyForWriting);
        }
        self.written.send(data.to_owned()).map_err(|_| Error::DriverStopped)
    }

    async fn read_message(&mut self) -> Result<Option<JsonObject>, Error> {
        match self.incoming.recv().await {
            Some(Incoming::Object(object)) => Ok(Some(object)),
            Some(Incoming::Failure(message)) => Err(Error::Transport(message.into())),
            None => Ok(None),
        }
    }

    async fn close(&mut self) -> Result<(), Error> {
        self.calls.record("close");
        self.ready = false;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    async fn end_input(&mut self) -> Result<(), Error> {
        self.calls.record("end_input");
        self.ready = false;
        Ok(())
    }

    async fn kill(&mut self) -> Result<(), Error> {
        self.calls.record("kill");
        Ok(())
    }
}

/// The far end of a [`ScriptedTransport`]: what the SDK wrote and what the "CLI" says next.
struct Peer {
    written: mpsc::UnboundedReceiver<String>,
    incoming: mpsc::UnboundedSender<Incoming>,
    calls: Calls,
    fail_writes: Arc<AtomicBool>,
}

fn transport(refuse_connect: bool) -> (ScriptedTransport, Peer) {
    let (written_tx, written) = mpsc::unbounded_channel();
    let (incoming_tx, incoming) = mpsc::unbounded_channel();
    let calls = Calls::default();
    let fail_writes = Arc::new(AtomicBool::new(false));
    (
        ScriptedTransport {
            written: written_tx,
            incoming,
            calls: calls.clone(),
            refuse_connect,
            fail_writes: fail_writes.clone(),
            ready: true,
        },
        Peer {
            written,
            incoming: incoming_tx,
            calls,
            fail_writes,
        },
    )
}

impl Peer {
    /// The next document the SDK wrote: one JSON object ending with a newline, as Python's `Query` writes it.
    async fn read(&mut self) -> Result<Value, Box<dyn StdError>> {
        let text = timeout(Duration::from_secs(5), self.written.recv())
            .await?
            .ok_or_else(|| std::io::Error::other("the transport was dropped"))?;
        assert!(text.ends_with('\n'), "a document must end with a newline: {text:?}");
        assert_eq!(text.matches('\n').count(), 1);
        Ok(serde_json::from_str(&text)?)
    }

    fn send(&self, value: Value) -> TestResult {
        let Value::Object(object) = value else {
            return Err(std::io::Error::other("not an object").into());
        };
        self.incoming
            .send(Incoming::Object(object))
            .map_err(|_| std::io::Error::other("the transport was dropped"))?;
        Ok(())
    }

    fn fail(&self, message: &str) -> TestResult {
        self.incoming
            .send(Incoming::Failure(message.to_owned()))
            .map_err(|_| std::io::Error::other("the transport was dropped"))?;
        Ok(())
    }

    fn respond(&self, request: &Value, response: Value) -> TestResult {
        self.send(json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": request["request_id"], "response": response},
        }))
    }

    /// Answers the initialize request the way the CLI does.
    async fn accept_initialize(&mut self) -> TestResult {
        let request = self.read().await?;
        assert_eq!(request["type"], "control_request");
        assert_eq!(request["request"]["subtype"], "initialize");
        self.respond(&request, json!({}))
    }
}

fn result(text: &str) -> Value {
    json!({
        "type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "duration_api_ms": 1,
        "num_turns": 1, "session_id": "scripted-session", "result": text, "total_cost_usd": 0.0,
    })
}

fn options() -> Result<ClaudeAgentOptions, Box<dyn StdError>> {
    let mut options = ClaudeAgentOptions::default();
    options.limits.control_timeout = Timeout::new(Duration::from_secs(5))?;
    Ok(options)
}

#[tokio::test]
async fn t15_a_custom_transport_drives_a_full_session_through_the_trait() -> TestResult {
    let (transport, mut peer) = transport(false);
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_transport(options()?, transport));
    peer.accept_initialize().await?;
    let mut client = connecting.await??;
    assert_eq!(client.process_id(), None);
    assert_eq!(peer.calls.all(), ["connect", "write"]);

    client.send_prompt(Prompt::from("hello")).await?;
    let user = peer.read().await?;
    assert_eq!(user["type"], "user");
    assert_eq!(user["message"]["content"], "hello");
    peer.send(result("done"))?;
    assert!(matches!(client.next_message().await?, Some(Message::Result(_))));

    client.finish_input().await?;
    client.disconnect().await?;
    assert_eq!(peer.calls.count("end_input"), 1);
    assert_eq!(peer.calls.count("close"), 1);
    assert_eq!(peer.calls.count("kill"), 0);
    Ok(())
}

#[tokio::test]
async fn t15_the_end_of_the_message_stream_ends_the_run() -> TestResult {
    let (transport, mut peer) = transport(false);
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_transport(options()?, transport));
    peer.accept_initialize().await?;
    let mut client = connecting.await??;
    client.send_prompt(Prompt::from("hello")).await?;
    peer.read().await?;
    peer.send(result("done"))?;
    assert!(matches!(client.next_message().await?, Some(Message::Result(_))));
    client.finish_input().await?;
    drop(peer.incoming);
    assert!(client.next_message().await?.is_none());
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn q01_query_with_a_transport_writes_the_prompt_after_initialize_and_ends_input() -> TestResult {
    let (transport, mut peer) = transport(false);
    let querying = tokio::spawn(query_with_transport(Prompt::from("what is 2+2"), options()?, transport));
    peer.accept_initialize().await?;
    let mut query = querying.await??;
    let user = peer.read().await?;
    assert_eq!(user["message"]["content"], "what is 2+2");
    peer.send(result("4"))?;
    assert!(matches!(query.next_message().await?, Some(Message::Result(_))));
    drop(peer.incoming);
    assert!(query.next_message().await?.is_none());
    query.close().await?;
    assert_eq!(peer.calls.count("connect"), 1);
    assert_eq!(peer.calls.count("end_input"), 1);
    assert_eq!(peer.calls.count("close"), 1);
    Ok(())
}

#[tokio::test]
async fn q01_a_one_turn_query_stops_after_the_first_result() -> TestResult {
    let (transport, mut peer) = transport(false);
    let querying = tokio::spawn(query_turn_with_transport(Prompt::from("hi"), options()?, transport));
    peer.accept_initialize().await?;
    let mut query = querying.await??;
    peer.read().await?;
    peer.send(result("first"))?;
    assert!(matches!(query.next_message().await?, Some(Message::Result(_))));
    assert!(query.next_message().await?.is_none());
    query.close().await?;
    assert_eq!(peer.calls.count("close"), 1);
    Ok(())
}

#[tokio::test]
async fn t15_a_transport_error_from_connect_is_returned_unchanged() -> TestResult {
    let (transport, _peer) = transport(true);
    let error = ClaudeSdkClient::connect_with_transport(options()?, transport)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("connect must fail"))?;
    assert!(matches!(error, Error::Transport(_)), "{error:?}");
    assert_eq!(error.to_string(), "connection refused");
    Ok(())
}

#[tokio::test]
async fn t15_a_transport_error_from_read_ends_the_run_and_kills_the_peer() -> TestResult {
    let (transport, mut peer) = transport(false);
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_transport(options()?, transport));
    peer.accept_initialize().await?;
    let mut client = connecting.await??;
    peer.fail("socket reset")?;
    let error = client
        .next_message()
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("the read error must surface"))?;
    assert!(matches!(error, Error::Transport(_)), "{error:?}");
    assert_eq!(error.to_string(), "socket reset");
    let _ = client.disconnect().await;
    let calls = peer.calls.all();
    let kill = calls.iter().position(|call| *call == "kill");
    let close = calls.iter().position(|call| *call == "close");
    assert!(kill.is_some() && kill < close, "kill precedes close: {calls:?}");
    Ok(())
}

#[tokio::test]
async fn t15_a_write_error_from_the_transport_reaches_the_caller() -> TestResult {
    let (transport, mut peer) = transport(false);
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_transport(options()?, transport));
    peer.accept_initialize().await?;
    let mut client = connecting.await??;
    peer.fail_writes.store(true, Ordering::SeqCst);
    // The failed write ends the run, as in upstream, where it fails the whole query.
    let _ = client.send_prompt(Prompt::from("hello")).await;
    let error = client
        .next_message()
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("the write error must surface"))?;
    assert!(matches!(error, Error::NotReadyForWriting), "{error:?}");
    let _ = client.disconnect().await;
    Ok(())
}

#[tokio::test]
async fn t15_an_abort_kills_the_transport_before_closing_it() -> TestResult {
    let (transport, mut peer) = transport(false);
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_transport(options()?, transport));
    peer.accept_initialize().await?;
    let mut client = connecting.await??;
    client.abort().await?;
    let error = timeout(Duration::from_secs(5), async {
        loop {
            match client.next_message().await {
                Ok(Some(_)) => {}
                Ok(None) => return None,
                Err(error) => return Some(error),
            }
        }
    })
    .await?;
    assert!(matches!(error, Some(Error::Aborted(_))), "{error:?}");
    let _ = client.disconnect().await;
    let calls = peer.calls.all();
    let kill = calls.iter().position(|call| *call == "kill");
    let close = calls.iter().position(|call| *call == "close");
    assert!(kill.is_some() && kill < close, "kill precedes close: {calls:?}");
    Ok(())
}

struct CountingStore {
    inner: InMemorySessionStore,
    loads: AtomicUsize,
}

impl SessionStore for CountingStore {
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        self.inner.append(key, entries)
    }

    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.inner.load(key)
    }
}

#[tokio::test]
async fn t16_a_custom_transport_skips_resume_materialization_but_keeps_the_mirror() -> TestResult {
    let directory = tempfile::tempdir()?;
    let resumed = SessionId::from("11111111-1111-4111-8111-111111111111");
    let project = SessionProjectKey::new("scripted-project")?;
    let store = Arc::new(CountingStore {
        inner: InMemorySessionStore::default(),
        loads: AtomicUsize::new(0),
    });
    let Value::Object(earlier) = json!({"type": "user", "uuid": "old", "message": {"content": "earlier"}}) else {
        return Err(std::io::Error::other("not an object").into());
    };
    store
        .inner
        .append(
            &SessionKey::new(project.clone(), resumed.clone())?,
            &[SessionEntry::new(earlier)],
        )
        .await?;
    let mut store_options = SessionStoreOptions::from(store.clone() as Arc<dyn SessionStore>);
    store_options.config_directory = Some(directory.path().to_path_buf());
    store_options.project_key = Some(project.clone());
    let mut configured = options()?;
    configured.session_store = Some(store_options);
    configured.resume = Some(resumed);

    let (transport, mut peer) = transport(false);
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_transport(configured, transport));
    peer.accept_initialize().await?;
    let mut client = connecting.await??;
    assert_eq!(
        store.loads.load(Ordering::SeqCst),
        0,
        "the store was loaded for a transport that never sees the materialized options"
    );
    assert!(
        std::fs::read_dir(directory.path())?.next().is_none(),
        "nothing is materialized into the configuration directory"
    );

    let path = directory
        .path()
        .join("projects/scripted-project/scripted-session.jsonl");
    peer.send(json!({
        "type": "transcript_mirror", "filePath": path,
        "entries": [{"type": "user", "uuid": "entry-1", "message": {"content": "one"}}],
    }))?;
    peer.send(result("mirrored"))?;
    loop {
        if matches!(client.next_message().await?, Some(Message::Result(_))) {
            break;
        }
    }
    let mirrored = store
        .inner
        .load(&SessionKey::new(project, SessionId::from("scripted-session"))?)
        .await?;
    assert_eq!(mirrored.map(|entries| entries.len()), Some(1));
    client.disconnect().await?;
    Ok(())
}

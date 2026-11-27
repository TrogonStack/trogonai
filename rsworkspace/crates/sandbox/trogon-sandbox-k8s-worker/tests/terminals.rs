use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_nats::{Client, Subscriber};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, oneshot};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{LeaseId, TerminalError, TerminalId, TerminalOutput, TerminalRequest, TerminalResponse};
use trogon_sandbox_k8s_worker::{K8sSshHost, K8sTerminals, K8sWorkerError, TerminalBackend};
use trogon_sandbox_protocol::subjects::{terminal_output, worker_lease_activity};
use trogon_sandbox_providers::{K8sOpsError, TtyPodExec};
use trogon_sandbox_ssh_gateway::SshTerminalHost;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The test's side of one terminal the fake backend started.
struct Session {
    stdin: DuplexStream,
    stdout: DuplexStream,
    resize: mpsc::Receiver<(u16, u16)>,
    exit: oneshot::Sender<i32>,
    argv: Vec<String>,
}

#[derive(Default)]
struct Backend {
    pods: Mutex<HashMap<String, String>>,
    refuse: Mutex<bool>,
    sessions: Mutex<Vec<Session>>,
    hangups: Mutex<Vec<String>>,
}

impl Backend {
    fn hold(&self, lease: &str) {
        self.pods
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(lease.into(), format!("pod-{lease}"));
    }

    fn take(&self) -> Result<Session, Box<dyn std::error::Error>> {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .ok_or_else(|| "no terminal was started".into())
    }
}

#[async_trait]
impl TerminalBackend for Backend {
    async fn pod_for(&self, lease_id: &LeaseId) -> Option<String> {
        self.pods
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(lease_id.as_str())
            .cloned()
    }

    async fn start(&self, _pod: &str, argv: Vec<String>, _size: (u16, u16)) -> Result<TtyPodExec, K8sWorkerError> {
        if *self.refuse.lock().unwrap_or_else(PoisonError::into_inner) {
            return Err(K8sOpsError::Unsupported("terminals").into());
        }
        let (stdin, stdin_far) = tokio::io::duplex(4096);
        let (stdout_far, stdout) = tokio::io::duplex(4096);
        let (resize_tx, resize) = mpsc::channel(4);
        let (exit, exit_rx) = oneshot::channel();
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Session {
                stdin: stdin_far,
                stdout: stdout_far,
                resize,
                exit,
                argv,
            });
        Ok(TtyPodExec {
            stdin: Box::pin(stdin),
            stdout: Box::pin(stdout),
            resize: resize_tx,
            completion: Box::pin(async move { exit_rx.await.map_err(|_| K8sOpsError::Unsupported("exit")) }),
        })
    }

    async fn hangup(&self, pod: &str, pid_file: &str) {
        self.hangups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{pod} {pid_file}"));
    }
}

fn request(lease: &str, terminal: &str, op: Value) -> Result<TerminalRequest, serde_json::Error> {
    let mut body = json!({"lease_id": lease, "terminal_id": terminal});
    if let (Some(fields), Some(extra)) = (body.as_object_mut(), op.as_object()) {
        fields.extend(extra.clone());
    }
    serde_json::from_value(body)
}

fn open(lease: &str, terminal: &str) -> Result<TerminalRequest, serde_json::Error> {
    request(lease, terminal, json!({"op": "open", "cols": 80, "rows": 24}))
}

fn input(lease: &str, terminal: &str, bytes: &[u8]) -> Result<TerminalRequest, serde_json::Error> {
    request(lease, terminal, json!({"op": "input", "data": STANDARD.encode(bytes)}))
}

async fn send(terminals: &Arc<K8sTerminals>, req: &TerminalRequest) -> Result<TerminalResponse, serde_json::Error> {
    Ok(terminals.handle(&serde_json::to_vec(req)?).await)
}

async fn next(sub: &mut Subscriber) -> Result<TerminalOutput, Box<dyn std::error::Error>> {
    let message = tokio::time::timeout(Duration::from_secs(10), sub.next())
        .await
        .map_err(|_| "timed out waiting for a terminal frame")?
        .ok_or("the subscription ended")?;
    Ok(serde_json::from_slice(&message.payload)?)
}

struct Fixture {
    _server: JetStreamTestServer,
    client: Client,
    backend: Arc<Backend>,
}

async fn fixture() -> Fixture {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    Fixture {
        _server: server,
        client,
        backend: Arc::new(Backend::default()),
    }
}

fn is_failed(response: &TerminalResponse, error: &TerminalError) -> bool {
    matches!(response, TerminalResponse::Failed(found) if found == error)
}

#[tokio::test]
async fn a_terminal_takes_input_resizes_and_closes() -> TestResult {
    let Fixture {
        client,
        backend,
        _server,
    } = fixture().await;
    let terminals = Arc::new(K8sTerminals::new(backend.clone(), client.clone(), 4096));
    let id = TerminalId::new("term-1")?;
    let mut output = client.subscribe(terminal_output(&id)).await?;
    let mut activity = client
        .subscribe(worker_lease_activity(&LeaseId::new("lease-a")))
        .await?;

    let invalid = terminals.handle(b"not json").await;
    assert!(matches!(
        invalid,
        TerminalResponse::Failed(TerminalError::InvalidRequest { .. })
    ));
    let no_pod = send(&terminals, &open("lease-a", "term-1")?).await?;
    assert!(matches!(
        no_pod,
        TerminalResponse::Failed(TerminalError::SpawnFailed { .. })
    ));
    assert!(!terminals.holds(&LeaseId::new("lease-a")).await);

    backend.hold("lease-a");
    assert!(terminals.holds(&LeaseId::new("lease-a")).await);
    *backend.refuse.lock().unwrap_or_else(PoisonError::into_inner) = true;
    let refused = send(&terminals, &open("lease-a", "term-1")?).await?;
    assert!(matches!(
        refused,
        TerminalResponse::Failed(TerminalError::SpawnFailed { .. })
    ));
    *backend.refuse.lock().unwrap_or_else(PoisonError::into_inner) = false;

    let opened = send(
        &terminals,
        &request(
            "lease-a",
            "term-1",
            json!({"op": "open", "cols": 80, "rows": 24, "cmd": ["bash"], "env": {"A": "b"}, "cwd": "/work"}),
        )?,
    )
    .await?;
    assert!(matches!(opened, TerminalResponse::Opened { .. }));
    let mut session = backend.take()?;
    assert!(session.argv.join(" ").contains("bash"));
    assert_eq!(terminals.open_count(&LeaseId::new("lease-a")), 1);

    let taken = send(&terminals, &open("lease-b", "term-1")?).await?;
    assert!(is_failed(&taken, &TerminalError::IdInUse));

    assert_eq!(
        send(&terminals, &input("lease-a", "term-1", b"ls\n")?).await?,
        TerminalResponse::Done
    );
    let mut typed = [0u8; 3];
    session.stdin.read_exact(&mut typed).await?;
    assert_eq!(&typed, b"ls\n");
    assert!(is_failed(
        &send(&terminals, &input("lease-a", "missing", b"x")?).await?,
        &TerminalError::NotFound
    ));
    assert!(is_failed(
        &send(&terminals, &input("lease-b", "term-1", b"x")?).await?,
        &TerminalError::IdInUse
    ));

    let resize = request("lease-a", "term-1", json!({"op": "resize", "cols": 100, "rows": 30}))?;
    assert_eq!(send(&terminals, &resize).await?, TerminalResponse::Done);
    assert_eq!(session.resize.recv().await, Some((100, 30)));
    let missing = request("lease-a", "missing", json!({"op": "resize", "cols": 100, "rows": 30}))?;
    assert!(is_failed(&send(&terminals, &missing).await?, &TerminalError::NotFound));

    session.stdout.write_all(b"hello").await?;
    assert!(matches!(next(&mut output).await?, TerminalOutput::Data { seq: 0, .. }));
    assert!(
        tokio::time::timeout(Duration::from_secs(10), activity.next())
            .await?
            .is_some()
    );
    session.stdout.write_all(b"again").await?;
    assert!(matches!(next(&mut output).await?, TerminalOutput::Data { seq: 1, .. }));

    let reattach = request(
        "lease-a",
        "term-1",
        json!({"op": "open", "cols": 80, "rows": 24, "resume_from": 0}),
    )?;
    assert!(matches!(
        send(&terminals, &reattach).await?,
        TerminalResponse::Opened { .. }
    ));
    assert!(matches!(next(&mut output).await?, TerminalOutput::Data { .. }));
    let plain = open("lease-a", "term-1")?;
    assert!(matches!(
        send(&terminals, &plain).await?,
        TerminalResponse::Opened { .. }
    ));

    let close = request("lease-a", "term-1", json!({"op": "close"}))?;
    assert!(is_failed(
        &send(&terminals, &request("lease-b", "term-1", json!({"op": "close"}))?).await?,
        &TerminalError::IdInUse
    ));
    assert_eq!(send(&terminals, &close).await?, TerminalResponse::Done);
    while !matches!(next(&mut output).await?, TerminalOutput::Exit { .. }) {}
    assert_eq!(terminals.open_count(&LeaseId::new("lease-a")), 0);
    assert_eq!(backend.hangups.lock().unwrap_or_else(PoisonError::into_inner).len(), 1);
    assert_eq!(
        send(&terminals, &close).await?,
        TerminalResponse::Done,
        "closing a closed terminal is fine"
    );
    Ok(())
}

#[tokio::test]
async fn a_terminal_that_stopped_answering_reports_it() -> TestResult {
    let Fixture {
        client,
        backend,
        _server,
    } = fixture().await;
    let terminals = Arc::new(K8sTerminals::new(backend.clone(), client, 4096));
    backend.hold("lease-a");
    send(&terminals, &open("lease-a", "term-1")?).await?;
    let session = backend.take()?;
    drop(session.stdin);
    drop(session.resize);
    assert!(is_failed(
        &send(&terminals, &input("lease-a", "term-1", b"x")?).await?,
        &TerminalError::Exited
    ));
    let resize = request("lease-a", "term-1", json!({"op": "resize", "cols": 1, "rows": 1}))?;
    assert!(is_failed(&send(&terminals, &resize).await?, &TerminalError::Exited));
    Ok(())
}

#[tokio::test]
async fn a_terminal_that_exits_reports_its_code() -> TestResult {
    let Fixture {
        client,
        backend,
        _server,
    } = fixture().await;
    let terminals = Arc::new(K8sTerminals::new(backend.clone(), client.clone(), 4096));
    backend.hold("lease-a");
    let coded = TerminalId::new("coded")?;
    let unknown = TerminalId::new("unknown")?;
    let mut coded_output = client.subscribe(terminal_output(&coded)).await?;
    let mut unknown_output = client.subscribe(terminal_output(&unknown)).await?;

    send(&terminals, &open("lease-a", "coded")?).await?;
    let session = backend.take()?;
    session.exit.send(7).map_err(|_| "the exit was not awaited")?;
    drop(session.stdout);
    assert_eq!(
        next(&mut coded_output).await?,
        TerminalOutput::Exit {
            seq: 0,
            exit_code: Some(7)
        }
    );

    send(&terminals, &open("lease-a", "unknown")?).await?;
    let session = backend.take()?;
    drop(session.exit);
    drop(session.stdout);
    assert_eq!(
        next(&mut unknown_output).await?,
        TerminalOutput::Exit {
            seq: 0,
            exit_code: None
        }
    );
    assert_eq!(terminals.open_count(&LeaseId::new("lease-a")), 0);
    assert!(
        backend
            .hangups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn a_lease_holds_a_bounded_number_of_terminals_and_releases_them() -> TestResult {
    let Fixture {
        client,
        backend,
        _server,
    } = fixture().await;
    let terminals = Arc::new(K8sTerminals::new(backend.clone(), client, 4096));
    backend.hold("lease-a");
    let mut sessions = Vec::new();
    for index in 0..trogon_sandbox_protocol::MAX_TERMINALS_PER_LEASE {
        let opened = send(&terminals, &open("lease-a", &format!("term-{index}"))?).await?;
        assert!(matches!(opened, TerminalResponse::Opened { .. }));
        sessions.push(backend.take()?);
    }
    let over = send(&terminals, &open("lease-a", "one-too-many")?).await?;
    assert!(matches!(
        over,
        TerminalResponse::Failed(TerminalError::TooManyTerminals { .. })
    ));

    terminals.close_lease(&LeaseId::new("lease-a")).await;
    assert_eq!(terminals.open_count(&LeaseId::new("lease-a")), 0);
    assert!(
        backend
            .hangups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty(),
        "a released lease needs no signal"
    );
    Ok(())
}

#[tokio::test]
async fn an_idle_terminal_is_closed_unless_it_is_used() -> TestResult {
    let Fixture {
        client,
        backend,
        _server,
    } = fixture().await;
    let terminals = Arc::new(K8sTerminals::with_idle_timeout(
        backend.clone(),
        client.clone(),
        4096,
        Duration::from_millis(600),
    ));
    let id = TerminalId::new("term-idle")?;
    let mut output = client.subscribe(terminal_output(&id)).await?;
    backend.hold("lease-a");
    send(&terminals, &open("lease-a", "term-idle")?).await?;
    let _session = backend.take()?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send(&terminals, &input("lease-a", "term-idle", b"x")?).await?;
    assert!(matches!(
        next(&mut output).await?,
        TerminalOutput::Exit { exit_code: None, .. }
    ));
    assert_eq!(backend.hangups.lock().unwrap_or_else(PoisonError::into_inner).len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_ssh_host_forwards_to_the_terminals() -> TestResult {
    let Fixture {
        client,
        backend,
        _server,
    } = fixture().await;
    let host = K8sSshHost(Arc::new(K8sTerminals::new(backend.clone(), client, 4096)));
    backend.hold("lease-a");
    assert!(host.holds(&LeaseId::new("lease-a")).await);
    assert!(!host.holds(&LeaseId::new("lease-z")).await);
    assert!(matches!(
        host.terminal(&open("lease-a", "term-ssh")?).await,
        TerminalResponse::Opened { .. }
    ));
    Ok(())
}

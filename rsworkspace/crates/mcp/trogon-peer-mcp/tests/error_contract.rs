#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::future::{Future as _, poll_fn};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};
use trogon_peer_mcp::{ConnectorConfigInput, EnrollPeer, PeerMcp, PreparePeerBatch};
use trogonai_proto::peers::v1alpha1 as v1;

#[derive(Clone, Copy)]
enum Work {
    Ready,
    Fail,
    Pending,
    Delay(Duration),
    Until(tokio::time::Instant),
}

#[derive(Debug, thiserror::Error)]
#[error("untrusted_backend_source_content")]
struct BackendError;

impl Work {
    async fn run(self) -> Result<(), BackendError> {
        match self {
            Self::Ready => Ok(()),
            Self::Fail => Err(BackendError),
            Self::Pending => std::future::pending().await,
            Self::Delay(delay) => {
                tokio::time::sleep(delay).await;
                Ok(())
            }
            Self::Until(instant) => {
                tokio::time::sleep_until(instant).await;
                Ok(())
            }
        }
    }
}

#[derive(Clone)]
struct Backend {
    enroll_work: Arc<Mutex<Work>>,
    prepare_work: Arc<Mutex<Work>>,
    enrollments: Arc<Mutex<Vec<v1::EnrollRequest>>>,
    preparations: Arc<Mutex<Vec<v1::PrepareBatchRequest>>>,
    enrollment: v1::EnrollResponse,
}

impl Backend {
    fn new(enroll_work: Work, prepare_work: Work) -> Self {
        Self {
            enroll_work: Arc::new(Mutex::new(enroll_work)),
            prepare_work: Arc::new(Mutex::new(prepare_work)),
            enrollments: Arc::default(),
            preparations: Arc::default(),
            enrollment: serde_json::from_value(json!({
                "peerId":"b".repeat(64),
                "attachment": {
                    "attachmentRef":{"peerId":"b".repeat(64),"connectorId":"connector_b","generation":1},
                    "status":"ATTACHMENT_STATUS_ATTACHED",
                    "observedAt":"2026-11-02T12:00:00Z", "expiresAt":"2026-11-02T12:00:10Z"
                }
            }))
            .expect("enrollment fixture"),
        }
    }
}

impl EnrollPeer for Backend {
    type Error = BackendError;

    async fn enroll(&self, request: v1::EnrollRequest) -> Result<v1::EnrollResponse, Self::Error> {
        self.enrollments.lock().expect("enrollments").push(request);
        let work = *self.enroll_work.lock().expect("enroll work");
        work.run().await?;
        Ok(self.enrollment.clone())
    }
}

impl PreparePeerBatch for Backend {
    type Error = BackendError;

    async fn prepare_batch(&self, request: v1::PrepareBatchRequest) -> Result<v1::PrepareBatchResponse, Self::Error> {
        self.preparations.lock().expect("preparations").push(request);
        let work = *self.prepare_work.lock().expect("prepare work");
        work.run().await?;
        Ok(Default::default())
    }
}

fn adapter(backend: Backend) -> PeerMcp<Backend> {
    PeerMcp::new(
        backend,
        ConnectorConfigInput {
            installation_id: "installation_b".into(),
            connector_id: "connector_b".into(),
            collaboration_ref: "collaboration_a".into(),
            display_name: "reviewer".into(),
        },
    )
    .expect("config")
}

fn request(name: &str, arguments: Value) -> CallToolRequestParams {
    serde_json::from_value(json!({"name":name, "arguments":arguments})).expect("request")
}

fn prompt(session: &str) -> CallToolRequestParams {
    request(
        "peer_prompt_hook",
        json!({
            "session_id":session, "prompt_id":"stable_prompt", "hook_event_name":"UserPromptSubmit"
        }),
    )
}

async fn served(backend: Backend, requests: Vec<CallToolRequestParams>) -> Vec<Value> {
    let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
    let server = tokio::spawn(async move {
        adapter(backend)
            .serve(server_transport)
            .await
            .expect("server")
            .waiting()
            .await
            .expect("shutdown")
    });
    let client = ().serve(client_transport).await.expect("client");
    let mut results = Vec::new();
    for request in requests {
        results.push(serde_json::to_value(client.call_tool(request).await.expect("tool result")).expect("wire JSON"));
    }
    client.cancel().await.expect("close client");
    server.await.expect("server task");
    results
}

fn assert_code(result: &Value, code: &str) {
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"], json!({"code":code}));
    let rendered = result.to_string();
    for private in ["untrusted_backend_source_content", "untrusted_argument_content"] {
        assert!(!rendered.contains(private), "served result exposed untrusted content");
    }
}

#[tokio::test]
async fn served_codes_distinguish_typed_failures_with_the_same_safe_message() {
    let results = served(
        Backend::new(Work::Ready, Work::Ready),
        vec![
            request(
                "peer_prompt_hook",
                json!({"session_id":{"untrusted_argument_content":true}}),
            ),
            prompt(""),
        ],
    )
    .await;
    assert_eq!(results[0]["content"], results[1]["content"]);
    assert_code(&results[0], "invalid_arguments");
    assert_code(&results[1], "invalid_host_identity");

    let enroll = served(Backend::new(Work::Fail, Work::Ready), vec![prompt("native_b")]).await;
    let prepare = served(Backend::new(Work::Ready, Work::Fail), vec![prompt("native_b")]).await;
    assert_eq!(enroll[0]["content"], prepare[0]["content"]);
    assert_code(&enroll[0], "enroll");
    assert_code(&prepare[0], "prepare");

    let mut missing = Backend::new(Work::Ready, Work::Ready);
    missing.enrollment.peer_id = None;
    let mut invalid = missing.clone();
    invalid.enrollment.peer_id = Some("untrusted_backend_source_content".into());
    let missing = served(missing, vec![prompt("native_b")]).await;
    let invalid = served(invalid, vec![prompt("native_b")]).await;
    assert_eq!(missing[0]["content"], invalid[0]["content"]);
    assert_code(&missing[0], "invalid_enrollment");
    assert_code(&invalid[0], "invalid_enrollment_identity");
}

#[tokio::test(start_paused = true)]
async fn prompt_deadline_bounds_pending_backend_and_preserves_identity_for_retry() {
    for (enroll_work, prepare_work) in [(Work::Pending, Work::Ready), (Work::Ready, Work::Pending)] {
        let backend = Backend::new(enroll_work, prepare_work);
        let connector = adapter(backend.clone());
        let started = tokio::time::Instant::now();
        let outcome = tokio::time::timeout(Duration::from_millis(2100), connector.invoke(prompt("native_b")))
            .await
            .expect("connector must enforce its own total deadline");
        assert!(outcome.is_err());
        assert_eq!(started.elapsed(), Duration::from_secs(2));
        assert!(matches!(
            connector.invoke(prompt("native_other")).await,
            Err(trogon_peer_mcp::PeerMcpError::NativeSessionChanged)
        ));
        *backend.enroll_work.lock().expect("enroll work") = Work::Ready;
        *backend.prepare_work.lock().expect("prepare work") = Work::Ready;
        connector.invoke(prompt("native_b")).await.expect("retry");
        let enrollments = backend.enrollments.lock().expect("enrollments");
        assert_eq!(enrollments.len(), 2);
        assert_eq!(enrollments[0], enrollments[1]);
        let preparations = backend.preparations.lock().expect("preparations");
        if preparations.len() == 2 {
            assert_eq!(preparations[0], preparations[1]);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn prompt_deadline_is_one_total_budget_for_enrollment_and_preparation() {
    let connector = adapter(Backend::new(
        Work::Delay(Duration::from_millis(1500)),
        Work::Delay(Duration::from_millis(1500)),
    ));
    let started = tokio::time::Instant::now();
    let outcome = connector.invoke(prompt("native_b")).await;
    assert!(outcome.is_err(), "backend steps must share the hook budget");
    assert_eq!(started.elapsed(), Duration::from_secs(2));
}

#[tokio::test(start_paused = true)]
async fn prompt_deadline_includes_lock_wait_without_starting_more_backend_work() {
    let backend = Backend::new(Work::Pending, Work::Ready);
    let connector = adapter(backend.clone());
    let mut first = Box::pin(connector.invoke(prompt("native_b")));
    poll_fn(|context| {
        assert!(first.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(backend.enrollments.lock().expect("enrollments").len(), 1);
    let started = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(Duration::from_millis(2100), connector.invoke(prompt("native_b")))
        .await
        .expect("connector deadline must include the binding lock wait");
    assert!(outcome.is_err());
    assert_eq!(started.elapsed(), Duration::from_secs(2));
    assert_eq!(backend.enrollments.lock().expect("enrollments").len(), 1);
    assert!(backend.preparations.lock().expect("preparations").is_empty());
}

#[tokio::test(start_paused = true)]
async fn prompt_completion_at_the_total_deadline_keeps_the_completed_result() {
    let connector = adapter(Backend::new(
        Work::Ready,
        Work::Until(tokio::time::Instant::now() + Duration::from_secs(2)),
    ));
    let started = tokio::time::Instant::now();
    let result = connector.invoke(prompt("native_b")).await.expect("completed prompt");
    assert_eq!(started.elapsed(), Duration::from_secs(2));
    assert_ne!(result.is_error, Some(true));
}

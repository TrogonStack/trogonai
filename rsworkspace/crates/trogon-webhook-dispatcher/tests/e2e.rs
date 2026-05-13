use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::jetstream;
use axum::{Router, body::Bytes, extract::State, http::StatusCode, routing::post};
use testcontainers_modules::{nats::Nats, testcontainers::{ImageExt, runners::AsyncRunner as _}};
use tokio::net::TcpListener;
use trogon_webhook_dispatcher::{
    Dispatcher, ReqwestWebhookClient, WebhookRegistry, provision,
};
use trogon_webhook_dispatcher::subscription::WebhookSubscription;

// ── Helper: spin up an echo HTTP server that records inbound POST bodies ──────

#[derive(Clone, Default)]
struct ReceivedHooks {
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    headers: Arc<Mutex<Vec<Vec<(String, String)>>>>,
}

impl ReceivedHooks {
    fn new() -> Self {
        Self::default()
    }

    fn take_bodies(&self) -> Vec<Vec<u8>> {
        self.bodies.lock().unwrap().drain(..).collect()
    }

    fn take_header_sets(&self) -> Vec<Vec<(String, String)>> {
        self.headers.lock().unwrap().drain(..).collect()
    }
}

async fn hook_receiver(
    State(received): State<ReceivedHooks>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    received.bodies.lock().unwrap().push(body.to_vec());
    let hdrs: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    received.headers.lock().unwrap().push(hdrs);
    StatusCode::OK
}

async fn spawn_hook_server(received: ReceivedHooks) -> SocketAddr {
    let app = Router::new()
        .route("/hook", post(hook_receiver))
        .with_state(received);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// ── Helper: provision stream + registry, register a subscription ──────────────

async fn setup(
    nats_url: &str,
    hook_url: &str,
    pattern: &str,
    secret: Option<&str>,
) -> (
    async_nats::Client,
    jetstream::Context,
    WebhookRegistry<async_nats::jetstream::kv::Store>,
    Dispatcher<
        jetstream::Context,
        async_nats::jetstream::kv::Store,
        ReqwestWebhookClient,
    >,
) {
    let nats = async_nats::connect(nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: "TRANSCRIPTS".to_string(),
        subjects: vec!["transcripts.>".to_string()],
        ..Default::default()
    })
    .await
    .unwrap();

    let kv = provision(&js).await.unwrap();
    let registry = WebhookRegistry::new(kv);

    registry
        .register(&WebhookSubscription {
            id: "e2e-sub".to_string(),
            subject_pattern: pattern.to_string(),
            url: format!("http://{hook_url}/hook"),
            secret: secret.map(str::to_string),
        })
        .await
        .unwrap();

    let http = ReqwestWebhookClient::new(Duration::from_secs(5));
    let dispatcher = Dispatcher::new(
        js.clone(),
        "TRANSCRIPTS".to_string(),
        "webhook-dispatcher-e2e".to_string(),
        "transcripts.>".to_string(),
        registry.clone(),
        http,
    );

    (nats, js, registry, dispatcher)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker"]
async fn dispatches_transcript_event_to_webhook() {
    let container = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats_url = format!("127.0.0.1:{port}");

    let received = ReceivedHooks::new();
    let hook_addr = spawn_hook_server(received.clone()).await;

    let (nats, js, _registry, dispatcher) =
        setup(&nats_url, &hook_addr.to_string(), "transcripts.>", None).await;

    tokio::spawn(async move { dispatcher.run().await.ok() });

    tokio::time::sleep(Duration::from_millis(100)).await;

    js.publish("transcripts.pr.owner.repo.1.sess-abc", br#"{"type":"message","role":"user","content":"review this"}"#.as_slice().into())
        .await
        .unwrap()
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let bodies = received.take_bodies();
            if !bodies.is_empty() {
                let payload: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
                assert_eq!(payload["subject"], "transcripts.pr.owner.repo.1.sess-abc");
                assert_eq!(payload["data"]["type"], "message");
                assert!(payload["timestamp_ms"].is_number());
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("timed out waiting for webhook delivery");

    drop(nats);
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn signed_delivery_includes_signature_header() {
    let container = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats_url = format!("127.0.0.1:{port}");

    let received = ReceivedHooks::new();
    let hook_addr = spawn_hook_server(received.clone()).await;

    let (_nats, js, _registry, dispatcher) =
        setup(&nats_url, &hook_addr.to_string(), "transcripts.>", Some("test-secret")).await;

    tokio::spawn(async move { dispatcher.run().await.ok() });

    tokio::time::sleep(Duration::from_millis(100)).await;

    js.publish("transcripts.pr.x.y.z.sess-1", b"{}".as_slice().into())
        .await
        .unwrap()
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let header_sets = received.take_header_sets();
            if !header_sets.is_empty() {
                let has_sig = header_sets[0]
                    .iter()
                    .any(|(k, v)| k.to_lowercase() == "x-trogon-signature-256" && v.starts_with("sha256="));
                assert!(has_sig, "missing X-Trogon-Signature-256 header");
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("timed out waiting for signed delivery");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn non_matching_subject_is_not_dispatched() {
    let container = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats_url = format!("127.0.0.1:{port}");

    let received = ReceivedHooks::new();
    let hook_addr = spawn_hook_server(received.clone()).await;

    let (_nats, js, _registry, dispatcher) =
        setup(&nats_url, &hook_addr.to_string(), "github.>", None).await;

    tokio::spawn(async move { dispatcher.run().await.ok() });

    tokio::time::sleep(Duration::from_millis(100)).await;

    js.publish("transcripts.pr.owner.repo.1.sess-xyz", b"{}".as_slice().into())
        .await
        .unwrap()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(received.take_bodies().is_empty(), "should not dispatch to github.> subscription");
}

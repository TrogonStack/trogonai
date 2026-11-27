//! Watch authentication across native captures: a parked source poll keeps working after the capture refreshes the
//! source's access token, and a restored fork keeps its own token independent from the source's.

mod watch_support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::sync::Notify;
use trogon_sandbox_contract::{
    ArtifactType, CheckpointRunningRequest, FileOp, FileOpResponse, LeaseId, NativeSnapshotRef, SandboxFreezeResponse,
    SandboxImage, SnapshotPushTarget,
};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};
use trogon_sandbox_protocol::Provider;
use watch_support::{Boxed, Served, StartedWatch, TestResult, WAIT, lease_request, locked, provider, serve, shape};

const SOURCE: &str = "lease-source";
const FORK_TEMPLATE: &str = "snapshot-watch:11111111-2222-4333-8444-555555555555";

#[derive(Default)]
struct Control {
    creates: AtomicUsize,
    generation: AtomicUsize,
    source_event: AtomicBool,
    fork_event: AtomicBool,
    poll_started: Notify,
    seen: Mutex<Vec<Value>>,
}

impl Control {
    fn record(&self, what: Value) {
        locked(&self.seen).push(what);
    }

    fn seen(&self) -> Vec<Value> {
        locked(&self.seen).clone()
    }
}

fn token(headers: &HeaderMap) -> Option<&str> {
    headers.get("x-access-token").and_then(|value| value.to_str().ok())
}

fn authorized(control: &Control, headers: &HeaderMap, fork: bool) -> bool {
    let expected = if fork {
        "fixture-fork".to_owned()
    } else {
        format!("fixture-source-{}", control.generation.load(Ordering::SeqCst))
    };
    token(headers) == Some(expected.as_str())
}

async fn create(State(control): State<Arc<Control>>, Json(body): Json<Value>) -> Json<Value> {
    if control.creates.fetch_add(1, Ordering::SeqCst) == 0 {
        control.record(body["templateID"].clone());
        Json(json!({"sandboxID": "source", "envdAccessToken": "fixture-source-0"}))
    } else {
        control.record(body["templateID"].clone());
        Json(json!({"sandboxID": "fork", "envdAccessToken": "fixture-fork"}))
    }
}

async fn connect(State(control): State<Arc<Control>>) -> Json<Value> {
    let generation = control.generation.fetch_add(1, Ordering::SeqCst) + 1;
    Json(json!({"sandboxID": "source", "envdAccessToken": format!("fixture-source-{generation}")}))
}

async fn snapshot(State(control): State<Arc<Control>>, Json(body): Json<Value>) -> Json<Value> {
    control.record(body);
    Json(json!({"snapshotID": "snapshot-watch:default", "names": []}))
}

async fn template() -> Json<Value> {
    Json(json!({
        "templateID": "snapshot-watch",
        "buildID": "11111111-2222-4333-8444-555555555555",
        "buildCount": 1,
        "builds": [{"buildID": "11111111-2222-4333-8444-555555555555", "status": "ready"}],
    }))
}

async fn process_start() -> impl IntoResponse {
    let value = json!({"event": {"end": {"exited": true, "exitCode": 0}}})
        .to_string()
        .into_bytes();
    let mut bytes = vec![0];
    bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
    bytes.extend(value);
    ([("content-type", "application/connect+json")], bytes)
}

async fn create_watcher(State(control): State<Arc<Control>>, headers: HeaderMap) -> Response {
    let fork = token(&headers) == Some("fixture-fork");
    if !authorized(&control, &headers, fork) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({"watcherId": if fork { "watch-fork" } else { "watch-source" }})).into_response()
}

async fn events(State(control): State<Arc<Control>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let fork = body["watcherId"] == "watch-fork";
    if !authorized(&control, &headers, fork) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !fork {
        control.poll_started.notify_one();
    }
    let pending = if fork {
        &control.fork_event
    } else {
        &control.source_event
    };
    let events = if pending.swap(false, Ordering::SeqCst) {
        vec![json!({"name": "changed.txt", "type": "EVENT_TYPE_WRITE", "entry": {"type": "FILE_TYPE_FILE"}})]
    } else {
        vec![]
    };
    Json(json!({"events": events})).into_response()
}

async fn remove_watcher() -> Json<Value> {
    Json(json!({}))
}

struct Rig {
    served: Served,
    provider: Arc<trogon_sandbox_e2b_worker::E2bWorkerProvider>,
    control: Arc<Control>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[derive(serde::Deserialize)]
struct Captured {
    reference: NativeSnapshotRef,
}

impl Rig {
    async fn new() -> Result<Self, Boxed> {
        let control = Arc::new(Control::default());
        let router = Router::new()
            .route("/v2/sandboxes", post(create))
            .route("/v2/sandboxes/source/connect", post(connect))
            .route("/sandboxes/source/snapshots", post(snapshot))
            .route("/templates/snapshot-watch", get(template))
            .route("/process.Process/Start", post(process_start))
            .route("/filesystem.Filesystem/CreateWatcher", post(create_watcher))
            .route("/filesystem.Filesystem/GetWatcherEvents", post(events))
            .route("/filesystem.Filesystem/RemoveWatcher", post(remove_watcher))
            .with_state(Arc::clone(&control));
        let (url, server) = serve(router).await?;
        let envd = Envd::new(Duration::from_secs(5))?;
        let provider = provider(&url, E2bExec::new(envd.clone()))?;
        provider
            .lease(&lease_request("watch-snapshot", None)?, &LeaseId::new(SOURCE))
            .await?;
        let served = Served::start(Arc::clone(&provider), envd, SOURCE).await?;
        Ok(Self {
            served,
            provider,
            control,
            server,
        })
    }

    async fn capture(&self) -> Result<NativeSnapshotRef, Boxed> {
        let request = CheckpointRunningRequest {
            lease_id: LeaseId::new(SOURCE),
            target: SnapshotPushTarget {
                registry: "unused.invalid".into(),
                repository: "snapshots".into(),
                artifact_type: ArtifactType::SandboxRootfsV1,
            },
            native_name: Some("watch-snapshot".into()),
        };
        let reply = tokio::time::timeout(
            WAIT,
            self.served.client.request(
                format!("sandbox.provider.e2b.checkpoint_running.{SOURCE}"),
                serde_json::to_vec(&request)?.into(),
            ),
        )
        .await??;
        Ok(serde_json::from_slice::<Captured>(&reply.payload)?.reference)
    }

    async fn start(&self, lease: &str) -> Result<String, Boxed> {
        let response = self
            .served
            .file_op(
                lease,
                FileOp::WatchStart {
                    path: "/work".into(),
                    recursive: true.into(),
                },
            )
            .await?;
        Ok(shape::<StartedWatch>(&response)?.watch_id)
    }
}

#[tokio::test]
async fn a_parked_watch_poll_uses_refreshed_source_auth_after_native_capture() -> TestResult {
    let rig = Rig::new().await?;
    let watch_id = rig.start(SOURCE).await?;
    let poll = rig.served.file_op(
        SOURCE,
        FileOp::WatchPoll {
            watch_id,
            after_seq: 0,
            wait_ms: Some(5000),
        },
    );
    let drive = async {
        tokio::time::timeout(Duration::from_secs(2), rig.control.poll_started.notified()).await?;
        rig.capture().await?;
        rig.control.source_event.store(true, Ordering::SeqCst);
        Ok::<(), Boxed>(())
    };
    let (response, driven) = tokio::join!(poll, drive);
    driven?;
    let polled: watch_support::PolledEvents = shape(&response?)?;
    assert_eq!(polled.events.len(), 1);
    assert_eq!(polled.events[0].path, "/work/changed.txt");
    assert_eq!(
        rig.control.seen(),
        vec![json!("base"), json!({"memory": true})],
        "the source was created from the base template and captured with memory"
    );
    Ok(())
}

#[tokio::test]
async fn restoring_a_fork_keeps_its_watch_auth_independent_from_source_refresh() -> TestResult {
    let rig = Rig::new().await?;
    let reference = rig.capture().await?;
    let request = lease_request("watch-fork", Some(SandboxImage::NativeSnapshot { reference }))?;
    rig.provider.lease(&request, &LeaseId::new("lease-fork")).await?;
    let watch_id = rig.start("lease-fork").await?;
    assert_eq!(
        rig.provider.set_paused_for_lease(&LeaseId::new(SOURCE), false).await,
        SandboxFreezeResponse::Done
    );
    rig.control.fork_event.store(true, Ordering::SeqCst);
    let response = rig
        .served
        .file_op(
            "lease-fork",
            FileOp::WatchPoll {
                watch_id,
                after_seq: 0,
                wait_ms: None,
            },
        )
        .await?;
    assert!(matches!(response, FileOpResponse::WatchEvents { events, .. } if events.len() == 1));
    assert_eq!(
        rig.control.seen(),
        vec![json!("base"), json!({"memory": true}), json!(FORK_TEMPLATE)],
        "the fork booted from the captured snapshot"
    );
    Ok(())
}

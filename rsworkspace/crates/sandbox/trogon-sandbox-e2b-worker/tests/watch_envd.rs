//! File watches driven over NATS end to end against an in-process fake envd: start, poll, stop, event type mapping,
//! the per-lease and per-watch bounds, and cleanup on release.
//!
//! The fake speaks the `filesystem.Filesystem` Connect JSON (`CreateWatcher`, `GetWatcherEvents`, `RemoveWatcher`).
//! It is not a live E2B sandbox; nothing here proves behavior against the real service.

mod watch_support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    FileKind, FileOp, FileOpErrorKind, FileOpResponse, FileRecursion, FileWatchEventKind, FileWatchTarget, LeaseId,
};
use trogon_sandbox_e2b_worker::{E2bExec, Envd, MAX_WATCHES_PER_LEASE, WATCH_BUFFER_EVENTS, Watches};
use trogon_sandbox_protocol::Provider;
use watch_support::{
    Boxed, PolledEvents, Served, StartedWatch, TestResult, WAIT, lease_request, locked, provider, serve, shape,
};

const LEASE: &str = "lease-1";

#[derive(Default)]
struct Fake {
    watchers: HashMap<String, Vec<Value>>,
    next: u32,
    creates: Vec<Value>,
    create_headers: Vec<(Option<String>, Option<String>)>,
    removed: Vec<String>,
    deleted: usize,
    sandbox_alive: bool,
}

type Shared = Arc<Mutex<Fake>>;

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(json!({"code": "not_found", "message": "no such watcher or directory"})),
    )
        .into_response()
}

async fn create_sandbox(State(fake): State<Shared>) -> impl IntoResponse {
    locked(&fake).sandbox_alive = true;
    (
        StatusCode::CREATED,
        axum::Json(json!({"sandboxID": "sbx1", "templateID": "base", "envdAccessToken": "envd-tok"})),
    )
}

async fn delete_sandbox(State(fake): State<Shared>) -> StatusCode {
    let mut fake = locked(&fake);
    fake.deleted += 1;
    fake.sandbox_alive = false;
    StatusCode::NO_CONTENT
}

async fn sandbox_detail(State(fake): State<Shared>) -> Response {
    if locked(&fake).sandbox_alive {
        axum::Json(json!({"state": "running"})).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

async fn create_watcher(
    State(fake): State<Shared>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<Value>,
) -> Response {
    if req["path"] == "/missing" {
        return not_found();
    }
    let mut fake = locked(&fake);
    fake.next += 1;
    let id = format!("envd-w{}", fake.next);
    fake.creates.push(req);
    fake.create_headers
        .push((header(&headers, "x-access-token"), header(&headers, "authorization")));
    fake.watchers.insert(id.clone(), Vec::new());
    axum::Json(json!({ "watcherId": id })).into_response()
}

async fn get_events(State(fake): State<Shared>, axum::Json(req): axum::Json<Value>) -> Response {
    let id = req["watcherId"].as_str().unwrap_or_default();
    match locked(&fake).watchers.get_mut(id) {
        Some(queue) => axum::Json(json!({ "events": std::mem::take(queue) })).into_response(),
        None => not_found(),
    }
}

async fn remove_watcher(State(fake): State<Shared>, axum::Json(req): axum::Json<Value>) -> Response {
    let id = req["watcherId"].as_str().unwrap_or_default().to_owned();
    let mut fake = locked(&fake);
    if fake.watchers.remove(&id).is_some() {
        fake.removed.push(id);
        axum::Json(json!({})).into_response()
    } else {
        not_found()
    }
}

async fn stat(axum::Json(body): axum::Json<Value>) -> axum::Json<Value> {
    axum::Json(json!({"entry": {"path": body["path"], "name": "w", "type": "FILE_TYPE_DIRECTORY", "size": "0"}}))
}

async fn list_dir() -> axum::Json<Value> {
    axum::Json(json!({"entries": []}))
}

struct Rig {
    served: Served,
    fake: Shared,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Rig {
    async fn new() -> Result<Self, Boxed> {
        let fake = Shared::default();
        let app = Router::new()
            .route("/v2/sandboxes", post(create_sandbox))
            .route("/sandboxes/{id}", get(sandbox_detail).delete(delete_sandbox))
            .route("/filesystem.Filesystem/CreateWatcher", post(create_watcher))
            .route("/filesystem.Filesystem/Stat", post(stat))
            .route("/filesystem.Filesystem/ListDir", post(list_dir))
            .route("/filesystem.Filesystem/GetWatcherEvents", post(get_events))
            .route("/filesystem.Filesystem/RemoveWatcher", post(remove_watcher))
            .with_state(Arc::clone(&fake));
        let (url, server) = serve(app).await?;
        let envd = Envd::new(Duration::from_secs(5))?;
        let watches = Watches::new(envd.clone()).with_poll_interval(Duration::from_millis(20));
        let provider = provider(&url, E2bExec::new(envd.clone()).with_watches(watches))?;
        provider
            .lease(&lease_request("watch", None)?, &LeaseId::new(LEASE))
            .await?;
        let served = Served::start(provider, envd, LEASE).await?;
        Ok(Self { served, fake, server })
    }

    async fn op(&self, op: FileOp) -> Result<FileOpResponse, Boxed> {
        self.served.file_op(LEASE, op).await
    }

    async fn start_watch(&self, path: &str, recursive: bool) -> Result<String, Boxed> {
        let response = self
            .op(FileOp::WatchStart {
                path: path.into(),
                recursive: recursive.into(),
            })
            .await?;
        Ok(shape::<StartedWatch>(&response)?.watch_id)
    }

    async fn poll_raw(&self, id: &str, after_seq: u64, wait_ms: Option<u32>) -> Result<FileOpResponse, Boxed> {
        self.op(FileOp::WatchPoll {
            watch_id: id.into(),
            after_seq,
            wait_ms,
        })
        .await
    }

    async fn poll(&self, id: &str, after_seq: u64, wait_ms: Option<u32>) -> Result<PolledEvents, Boxed> {
        shape(&self.poll_raw(id, after_seq, wait_ms).await?)
    }

    fn push(&self, events: Vec<Value>) -> TestResult {
        locked(&self.fake)
            .watchers
            .values_mut()
            .next()
            .ok_or("no envd watcher to push to")?
            .extend(events);
        Ok(())
    }
}

fn envd_event(name: &str, kind: &str, entry: Option<&str>) -> Value {
    let mut event = json!({ "name": name, "type": kind });
    if let Some(entry) = entry {
        event["entry"] = json!({ "name": name, "type": entry });
    }
    event
}

fn unknown_watch(response: &FileOpResponse) -> bool {
    matches!(
        response,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::UnknownWatch,
            ..
        }
    )
}

#[tokio::test]
async fn a_watch_starts_polls_maps_event_types_and_stops() -> TestResult {
    let rig = Rig::new().await?;
    let id = rig.start_watch("/w", true).await?;
    assert_eq!(id, "w-1");
    {
        let fake = locked(&rig.fake);
        assert_eq!(
            fake.creates[0],
            json!({"path": "/w", "recursive": true, "includeEntry": true})
        );
        // The sandbox's own token authenticates the call, never the API key.
        assert_eq!(fake.create_headers[0].0.as_deref(), Some("envd-tok"));
        assert_eq!(fake.create_headers[0].1.as_deref(), Some("Basic dXNlcjo="));
        assert!(!format!("{:?}", fake.create_headers).contains("api-key"));
    }

    let polled = rig.poll(&id, 0, None).await?;
    assert!(polled.events.is_empty() && polled.next_seq == 0 && !polled.overflowed.overflowed());

    rig.push(vec![
        envd_event("a.txt", "EVENT_TYPE_CREATE", Some("FILE_TYPE_FILE")),
        envd_event("d", "EVENT_TYPE_CREATE", Some("FILE_TYPE_DIRECTORY")),
        envd_event("a.txt", "EVENT_TYPE_WRITE", Some("FILE_TYPE_FILE")),
        envd_event("a.txt", "EVENT_TYPE_CHMOD", Some("FILE_TYPE_FILE")),
        envd_event("old.txt", "EVENT_TYPE_RENAME", None),
        envd_event("gone.txt", "EVENT_TYPE_REMOVE", None),
        envd_event("x", "EVENT_TYPE_UNSPECIFIED", None),
    ])?;
    let polled = rig.poll(&id, 0, None).await?;
    let shape: Vec<_> = polled
        .events
        .iter()
        .map(|e| (e.seq, e.kind, e.path.as_str(), e.is_dir))
        .collect();
    assert_eq!(
        shape,
        vec![
            (1, FileWatchEventKind::Created, "/w/a.txt", FileWatchTarget::File),
            (2, FileWatchEventKind::Created, "/w/d", FileWatchTarget::Directory),
            (3, FileWatchEventKind::Modified, "/w/a.txt", FileWatchTarget::File),
            (4, FileWatchEventKind::Modified, "/w/a.txt", FileWatchTarget::File),
            (5, FileWatchEventKind::Deleted, "/w/old.txt", FileWatchTarget::File),
            (6, FileWatchEventKind::Deleted, "/w/gone.txt", FileWatchTarget::File),
        ]
    );
    assert_eq!(polled.next_seq, 6);
    assert!(!polled.overflowed.overflowed());

    // A poll resumes after its cursor and never repeats an event.
    let rest = rig.poll(&id, 4, None).await?;
    assert_eq!(rest.events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![5, 6]);
    assert_eq!(rest.next_seq, 6);

    let stop = FileOp::WatchStop { watch_id: id.clone() };
    assert_eq!(
        rig.op(stop.clone()).await?,
        FileOpResponse::WatchStopped { existed: true.into() }
    );
    assert_eq!(locked(&rig.fake).removed, vec!["envd-w1"]);
    assert_eq!(
        rig.op(stop).await?,
        FileOpResponse::WatchStopped { existed: false.into() }
    );
    assert!(unknown_watch(&rig.poll_raw(&id, 0, None).await?));
    Ok(())
}

#[tokio::test]
async fn a_parked_poll_returns_when_an_event_arrives() -> TestResult {
    let rig = Rig::new().await?;
    let id = rig.start_watch("/w", false).await?;
    let polled = tokio::time::timeout(WAIT, async {
        let parked = rig.poll(&id, 0, Some(15_000));
        let arrival = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            rig.push(vec![envd_event("n", "EVENT_TYPE_CREATE", Some("FILE_TYPE_FILE"))])
        };
        let (polled, pushed) = tokio::join!(parked, arrival);
        pushed?;
        polled
    })
    .await??;
    assert_eq!(polled.events[0].path, "/w/n");
    assert_eq!(locked(&rig.fake).creates[0]["recursive"], false);
    Ok(())
}

#[tokio::test]
async fn a_poll_with_a_short_wait_gives_up_empty() -> TestResult {
    let rig = Rig::new().await?;
    let id = rig.start_watch("/w", false).await?;
    let started = std::time::Instant::now();
    let polled = rig.poll(&id, 7, Some(150)).await?;
    assert!(polled.events.is_empty());
    assert_eq!(polled.next_seq, 7);
    assert!(started.elapsed() >= Duration::from_millis(100));
    Ok(())
}

#[tokio::test]
async fn watches_per_lease_are_bounded_and_bad_paths_are_typed() -> TestResult {
    let rig = Rig::new().await?;
    for _ in 0..MAX_WATCHES_PER_LEASE {
        rig.start_watch("/w", false).await?;
    }
    let refused = rig
        .op(FileOp::WatchStart {
            path: "/w".into(),
            recursive: FileRecursion::Flat,
        })
        .await?;
    assert!(
        matches!(
            refused,
            FileOpResponse::Failed {
                kind: FileOpErrorKind::TooLarge,
                ..
            }
        ),
        "{refused:?}"
    );
    assert_eq!(
        locked(&rig.fake).watchers.len(),
        MAX_WATCHES_PER_LEASE,
        "a refused start must not leave an envd watcher behind"
    );

    // Stopping one frees a slot.
    rig.op(FileOp::WatchStop { watch_id: "w-1".into() }).await?;
    rig.start_watch("/w", false).await?;

    let relative = rig
        .op(FileOp::WatchStart {
            path: "rel".into(),
            recursive: FileRecursion::Flat,
        })
        .await?;
    assert!(matches!(
        relative,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::InvalidPath,
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn a_missing_directory_is_not_found() -> TestResult {
    let rig = Rig::new().await?;
    let response = rig
        .op(FileOp::WatchStart {
            path: "/missing".into(),
            recursive: FileRecursion::Flat,
        })
        .await?;
    assert_eq!(
        response,
        FileOpResponse::NotFound {
            path: "/missing".into()
        }
    );
    Ok(())
}

fn writes(from: usize, to: usize) -> Vec<Value> {
    (from..to)
        .map(|i| envd_event(&format!("f{i}"), "EVENT_TYPE_WRITE", None))
        .collect()
}

#[tokio::test]
async fn a_full_buffer_drops_the_oldest_and_reports_overflow() -> TestResult {
    let rig = Rig::new().await?;
    let id = rig.start_watch("/w", false).await?;
    // Two fetches under the per-fetch ceiling that together pass the buffer.
    rig.push(writes(0, 3000))?;
    rig.poll(&id, 0, None).await?;
    rig.push(writes(3000, 6000))?;
    let polled = rig.poll(&id, 0, None).await?;
    let dropped = 6000 - WATCH_BUFFER_EVENTS;
    assert!(polled.overflowed.overflowed());
    assert_eq!(polled.events.len(), FileOp::WATCH_MAX_EVENTS_PER_POLL);
    assert_eq!(polled.events[0].seq, dropped as u64 + 1, "the oldest were dropped");
    // A caller that kept up past the loss is not told about it again.
    let caught_up = rig.poll(&id, dropped as u64, Some(0)).await?;
    assert!(!caught_up.overflowed.overflowed());
    Ok(())
}

#[tokio::test]
async fn an_oversized_envd_batch_is_cut_and_reported() -> TestResult {
    let rig = Rig::new().await?;
    let id = rig.start_watch("/w", false).await?;
    rig.push(writes(0, 4100))?;
    let polled = rig.poll(&id, 0, None).await?;
    assert!(
        polled.overflowed.overflowed(),
        "events past the per-fetch ceiling were lost"
    );
    assert_eq!(polled.events.len(), FileOp::WATCH_MAX_EVENTS_PER_POLL);
    Ok(())
}

#[tokio::test]
async fn a_watcher_envd_lost_becomes_an_unknown_watch() -> TestResult {
    let rig = Rig::new().await?;
    let id = rig.start_watch("/w", false).await?;
    locked(&rig.fake).watchers.clear();
    for _ in 0..2 {
        assert!(unknown_watch(&rig.poll_raw(&id, 0, None).await?));
    }
    Ok(())
}

#[tokio::test]
async fn the_request_user_authenticates_the_watch() -> TestResult {
    let rig = Rig::new().await?;
    let response = rig
        .served
        .file_op_as(
            LEASE,
            FileOp::WatchStart {
                path: "/root".into(),
                recursive: FileRecursion::Flat,
            },
            Some("root"),
        )
        .await?;
    assert!(shape::<StartedWatch>(&response).is_ok());
    // Basic auth carries the user name, as `root:`.
    assert_eq!(locked(&rig.fake).create_headers[0].1.as_deref(), Some("Basic cm9vdDo="));
    Ok(())
}

#[tokio::test]
async fn general_file_verbs_do_not_create_watchers() -> TestResult {
    let rig = Rig::new().await?;
    let stat = shape::<StatOf>(&rig.op(FileOp::Stat { path: "/w".into() }).await?)?.stat;
    assert_eq!(stat.path, "/w");
    assert_eq!(stat.kind, FileKind::Directory);
    let listing = rig
        .op(FileOp::List {
            path: "/w".into(),
            recursive: FileRecursion::Flat,
            max_entries: None,
        })
        .await?;
    assert_eq!(
        listing,
        FileOpResponse::Listing {
            entries: vec![],
            truncated: false.into()
        }
    );
    assert!(locked(&rig.fake).creates.is_empty());
    Ok(())
}

#[derive(serde::Deserialize)]
struct StatOf {
    stat: trogon_sandbox_contract::FileStat,
}

#[tokio::test]
async fn releasing_the_lease_removes_every_watcher_and_ends_parked_polls() -> TestResult {
    let rig = Rig::new().await?;
    let first = rig.start_watch("/a", false).await?;
    rig.start_watch("/b", true).await?;
    let parked = rig.poll_raw(&first, 0, Some(20_000));
    let release = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        rig.served
            .client
            .publish(format!("sandbox.provider.e2b.cancel.{LEASE}"), "".into())
            .await
    };
    let (response, released) = tokio::time::timeout(WAIT, async { tokio::join!(parked, release) }).await?;
    released?;
    let response = response?;
    assert!(unknown_watch(&response), "{response:?}");
    tokio::time::timeout(WAIT, async {
        while {
            let fake = locked(&rig.fake);
            !(fake.removed.len() == 2 && fake.deleted == 1 && fake.watchers.is_empty())
        } {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

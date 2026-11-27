use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_nats::jetstream::stream;
use serde_json::{Value, json};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    FileOp, FileOpErrorKind, FileOpResponse, FilePresence, FileRecursion, FileWatchEvent, FileWatchEventKind,
    FileWatchOverflow, FileWatchTarget, LeaseId,
};
use url::Url;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::Watches;
use super::buffer::Buffer;
use super::constants::{MAX_WATCHES_PER_LEASE, MIN_POLL_INTERVAL, WATCH_BUFFER_EVENTS};
use super::event_path::event_path;
use crate::api::E2bConfig;
use crate::envd::{Envd, EnvdTarget, MAX_WATCHER_EVENTS_PER_FETCH, WatcherEvent};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const POLL: Duration = Duration::from_millis(5);

fn event(name: &str) -> WatcherEvent {
    WatcherEvent {
        name: name.to_owned(),
        kind: FileWatchEventKind::Created,
        is_dir: false,
    }
}

fn envd_for(server: &MockServer) -> Result<(Envd, EnvdTarget), Box<dyn std::error::Error>> {
    let config = E2bConfig::new(Url::parse("https://api.e2b.app")?, "e2b.app", "k")?
        .with_envd_url(Url::parse(&server.uri())?)?;
    let target = EnvdTarget::for_sandbox(&config, "sbx1", None, Some("tok".to_owned()))?;
    Ok((Envd::new(Duration::from_secs(5))?, target))
}

fn watches_for(server: &MockServer) -> Result<(Watches, EnvdTarget), Box<dyn std::error::Error>> {
    let (envd, target) = envd_for(server)?;
    Ok((Watches::new(envd).with_poll_interval(POLL), target))
}

fn lease() -> LeaseId {
    LeaseId::new("l1")
}

async fn run(watches: &Watches, target: &EnvdTarget, op: &FileOp) -> FileOpResponse {
    watches.serve(&lease(), target, None, op).await
}

fn start_op(path: &str) -> FileOp {
    FileOp::WatchStart {
        path: path.to_owned(),
        recursive: FileRecursion::Recursive,
    }
}

fn poll_op(watch_id: &str, after_seq: u64, wait_ms: Option<u32>) -> FileOp {
    FileOp::WatchPoll {
        watch_id: watch_id.to_owned(),
        after_seq,
        wait_ms,
    }
}

fn stop_op(watch_id: &str) -> FileOp {
    FileOp::WatchStop {
        watch_id: watch_id.to_owned(),
    }
}

async fn mount_create(server: &MockServer, id: &str) {
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"watcherId": id})))
        .mount(server)
        .await;
}

async fn mount_events(server: &MockServer, reply: Value, times: Option<u64>) {
    let mock = Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/GetWatcherEvents"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply));
    match times {
        Some(times) => mock.up_to_n_times(times).mount(server).await,
        None => mock.mount(server).await,
    }
}

fn created(name: &str) -> Value {
    json!({"name": name, "type": "EVENT_TYPE_CREATE", "entry": {"type": "FILE_TYPE_DIRECTORY"}})
}

async fn mount_remove(server: &MockServer, reply: ResponseTemplate, expected: u64) {
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/RemoveWatcher"))
        .respond_with(reply)
        .expect(expected)
        .mount(server)
        .await;
}

fn removed() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({}))
}

fn events_of(
    response: FileOpResponse,
) -> Result<(Vec<FileWatchEvent>, u64, FileWatchOverflow), Box<dyn std::error::Error>> {
    match response {
        FileOpResponse::WatchEvents {
            events,
            next_seq,
            overflowed,
        } => Ok((events, next_seq, overflowed)),
        other => Err(format!("watch events expected, got {other:?}").into()),
    }
}

fn failed_kind(response: &FileOpResponse) -> Option<FileOpErrorKind> {
    match response {
        FileOpResponse::Failed { kind, .. } => Some(*kind),
        _ => None,
    }
}

#[test]
fn event_names_join_onto_the_watched_directory() {
    assert_eq!(event_path("/w", "a.txt"), "/w/a.txt");
    assert_eq!(event_path("/w/", "./d/a"), "/w/d/a");
    assert_eq!(event_path("/w", "/abs/x"), "/abs/x");
    assert_eq!(event_path("/w", ""), "/w");
    assert_eq!(event_path("/", ""), "/");
    assert_eq!(event_path("/", "a"), "/a");
}

#[test]
fn the_buffer_drops_the_oldest_and_reports_the_loss() -> TestResult {
    let mut buffer = Buffer::new();
    let batch: Vec<WatcherEvent> = (0..WATCH_BUFFER_EVENTS + 3)
        .map(|index| event(&format!("f{index}")))
        .collect();
    buffer.push("/w", batch, 0);
    assert_eq!(buffer.events.len(), WATCH_BUFFER_EVENTS);
    assert_eq!(buffer.dropped_through, 3);
    let Some(response) = buffer.ready(0) else {
        return Err("events expected".into());
    };
    let (events, next_seq, overflowed) = events_of(response)?;
    assert_eq!(overflowed, FileWatchOverflow::Overflowed);
    assert_eq!(events.len(), FileOp::WATCH_MAX_EVENTS_PER_POLL);
    assert_eq!(events.first().map(|event| event.seq), Some(4));
    assert_eq!(Some(next_seq), events.last().map(|event| event.seq));
    // A caller already past the loss is not told about it again.
    let Some(response) = buffer.ready(3) else {
        return Err("events expected".into());
    };
    assert_eq!(events_of(response)?.2, FileWatchOverflow::Intact);
    Ok(())
}

#[test]
fn events_past_the_fetch_ceiling_count_as_an_overflow() {
    let mut buffer = Buffer::new();
    buffer.push("/w", vec![event("a")], 5);
    assert_eq!(buffer.dropped_through, 1);
    assert!(buffer.ready(1).is_none());
}

#[test]
fn the_poll_interval_has_a_floor() -> TestResult {
    let server_less = Envd::new(Duration::from_secs(1))?;
    let watches = Watches::new(server_less).with_poll_interval(Duration::ZERO);
    assert_eq!(watches.poll_interval, MIN_POLL_INTERVAL);
    Ok(())
}

#[tokio::test]
async fn a_watch_is_started_polled_and_stopped() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    mount_events(&server, json!({"events": [created("a")]}), Some(1)).await;
    mount_events(&server, json!({"events": []}), None).await;
    mount_remove(&server, removed(), 1).await;
    let (watches, target) = watches_for(&server)?;

    let FileOpResponse::WatchStarted { watch_id } = run(&watches, &target, &start_op("/w")).await else {
        return Err("a started watch expected".into());
    };
    assert_eq!(watch_id, "w-1");
    assert_eq!(watches.count("l1"), 1);

    let (events, next_seq, overflowed) = events_of(run(&watches, &target, &poll_op(&watch_id, 0, None)).await)?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].path, "/w/a");
    assert_eq!(events[0].kind, FileWatchEventKind::Created);
    assert_eq!(events[0].is_dir, FileWatchTarget::Directory);
    assert_eq!((next_seq, overflowed), (1, FileWatchOverflow::Intact));

    let (events, next_seq, _) = events_of(run(&watches, &target, &poll_op(&watch_id, 1, Some(0))).await)?;
    assert!(events.is_empty());
    assert_eq!(next_seq, 1);

    assert_eq!(
        run(&watches, &target, &stop_op(&watch_id)).await,
        FileOpResponse::WatchStopped {
            existed: FilePresence::Present
        }
    );
    assert_eq!(
        run(&watches, &target, &stop_op(&watch_id)).await,
        FileOpResponse::WatchStopped {
            existed: FilePresence::Absent
        }
    );
    let after = run(&watches, &target, &poll_op(&watch_id, 0, None)).await;
    assert_eq!(failed_kind(&after), Some(FileOpErrorKind::UnknownWatch));
    Ok(())
}

#[tokio::test]
async fn a_watch_runs_as_the_requested_user() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .and(header("Authorization", "Basic cm9vdDo="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"watcherId": "e1"})))
        .expect(1)
        .mount(&server)
        .await;
    let (watches, target) = watches_for(&server)?;
    let response = watches.serve(&lease(), &target, Some("root"), &start_op("/w")).await;
    assert!(matches!(response, FileOpResponse::WatchStarted { .. }));
    Ok(())
}

#[tokio::test]
async fn a_relative_watch_path_is_refused() -> TestResult {
    let server = MockServer::start().await;
    let (watches, target) = watches_for(&server)?;
    let response = run(&watches, &target, &start_op("w")).await;
    assert_eq!(failed_kind(&response), Some(FileOpErrorKind::InvalidPath));
    Ok(())
}

#[tokio::test]
async fn a_watch_envd_cannot_create_fails_with_the_envd_reason() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"code": "not_found", "message": "gone"})))
        .mount(&server)
        .await;
    let (watches, target) = watches_for(&server)?;
    assert_eq!(
        run(&watches, &target, &start_op("/gone")).await,
        FileOpResponse::NotFound {
            path: "/gone".to_owned()
        }
    );
    assert_eq!(watches.count("l1"), 0);
    Ok(())
}

#[tokio::test]
async fn an_unauthorized_or_rejected_envd_call_is_mapped_by_cause() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .and(|request: &Request| String::from_utf8_lossy(&request.body).contains("/denied"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"code": "unauthenticated", "message": "no"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"code": "invalid_argument", "message": "bad"})))
        .mount(&server)
        .await;
    let (watches, target) = watches_for(&server)?;
    let denied = run(&watches, &target, &start_op("/denied")).await;
    assert_eq!(failed_kind(&denied), Some(FileOpErrorKind::PermissionDenied));
    let rejected = run(&watches, &target, &start_op("/other")).await;
    assert_eq!(failed_kind(&rejected), Some(FileOpErrorKind::InvalidRequest));
    Ok(())
}

#[tokio::test]
async fn a_lease_may_hold_a_bounded_number_of_watches() -> TestResult {
    let server = MockServer::start().await;
    let created_so_far = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .respond_with(move |_: &Request| {
            let id = created_so_far.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(json!({"watcherId": format!("e{id}")}))
        })
        .mount(&server)
        .await;
    let (watches, target) = watches_for(&server)?;
    for _ in 0..MAX_WATCHES_PER_LEASE {
        let response = run(&watches, &target, &start_op("/w")).await;
        assert!(matches!(response, FileOpResponse::WatchStarted { .. }));
    }
    let refused = run(&watches, &target, &start_op("/w")).await;
    assert_eq!(failed_kind(&refused), Some(FileOpErrorKind::TooLarge));
    assert_eq!(watches.count("l1"), MAX_WATCHES_PER_LEASE);
    Ok(())
}

#[tokio::test]
async fn a_start_that_loses_the_last_slot_to_a_concurrent_start_removes_its_watcher() -> TestResult {
    let server = MockServer::start().await;
    let created_so_far = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/CreateWatcher"))
        .respond_with(move |_: &Request| {
            let id = created_so_far.fetch_add(1, Ordering::SeqCst);
            let reply = ResponseTemplate::new(200).set_body_json(json!({"watcherId": format!("e{id}")}));
            if id < MAX_WATCHES_PER_LEASE - 1 {
                reply
            } else {
                reply.set_delay(Duration::from_millis(150))
            }
        })
        .mount(&server)
        .await;
    mount_remove(&server, removed(), 1).await;
    let (watches, target) = watches_for(&server)?;
    for _ in 0..MAX_WATCHES_PER_LEASE - 1 {
        run(&watches, &target, &start_op("/w")).await;
    }
    let op = start_op("/w");
    let (first, second) = tokio::join!(run(&watches, &target, &op), run(&watches, &target, &op));
    let started = [&first, &second]
        .iter()
        .filter(|response| matches!(response, FileOpResponse::WatchStarted { .. }))
        .count();
    let refused = [&first, &second]
        .iter()
        .filter(|response| failed_kind(response) == Some(FileOpErrorKind::TooLarge))
        .count();
    assert_eq!((started, refused), (1, 1));
    assert_eq!(watches.count("l1"), MAX_WATCHES_PER_LEASE);
    Ok(())
}

#[tokio::test]
async fn polling_an_unknown_watch_is_refused() -> TestResult {
    let server = MockServer::start().await;
    let (watches, target) = watches_for(&server)?;
    let response = run(&watches, &target, &poll_op("w-9", 0, None)).await;
    assert_eq!(failed_kind(&response), Some(FileOpErrorKind::UnknownWatch));
    Ok(())
}

#[tokio::test]
async fn a_parked_poll_returns_when_an_event_arrives() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    mount_events(&server, json!({"events": []}), Some(2)).await;
    mount_events(&server, json!({"events": [created("late")]}), None).await;
    let (watches, target) = watches_for(&server)?;
    run(&watches, &target, &start_op("/w")).await;
    let (events, _, _) = events_of(run(&watches, &target, &poll_op("w-1", 0, Some(5_000))).await)?;
    assert_eq!(events.first().map(|event| event.path.as_str()), Some("/w/late"));
    Ok(())
}

#[tokio::test]
async fn a_poll_with_nothing_new_returns_empty_after_its_wait() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    mount_events(&server, json!({"events": []}), None).await;
    let (watches, target) = watches_for(&server)?;
    run(&watches, &target, &start_op("/w")).await;
    let (events, next_seq, overflowed) = events_of(run(&watches, &target, &poll_op("w-1", 7, Some(30))).await)?;
    assert!(events.is_empty());
    assert_eq!((next_seq, overflowed), (7, FileWatchOverflow::Intact));
    Ok(())
}

#[tokio::test]
async fn stopping_a_watch_ends_a_poll_parked_on_it() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    mount_events(&server, json!({"events": []}), None).await;
    mount_remove(&server, removed(), 1).await;
    let (watches, target) = watches_for(&server)?;
    run(&watches, &target, &start_op("/w")).await;
    let park = poll_op("w-1", 0, Some(10_000));
    let stop = stop_op("w-1");
    let (parked, stopped) = tokio::join!(run(&watches, &target, &park), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        run(&watches, &target, &stop).await
    });
    assert_eq!(failed_kind(&parked), Some(FileOpErrorKind::UnknownWatch));
    assert_eq!(
        stopped,
        FileOpResponse::WatchStopped {
            existed: FilePresence::Present
        }
    );
    Ok(())
}

#[tokio::test]
async fn forgetting_a_lease_ends_a_parked_poll_without_calling_envd() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    mount_events(&server, json!({"events": []}), None).await;
    mount_remove(&server, removed(), 0).await;
    let (watches, target) = watches_for(&server)?;
    run(&watches, &target, &start_op("/w")).await;
    let park = poll_op("w-1", 0, Some(10_000));
    let (parked, ()) = tokio::join!(run(&watches, &target, &park), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        watches.forget_lease("l1");
    });
    assert_eq!(failed_kind(&parked), Some(FileOpErrorKind::UnknownWatch));
    assert_eq!(watches.count("l1"), 0);
    Ok(())
}

#[tokio::test]
async fn a_watcher_envd_lost_makes_the_watch_unknown() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/GetWatcherEvents"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"code": "not_found", "message": "no watcher"})))
        .mount(&server)
        .await;
    let (watches, target) = watches_for(&server)?;
    run(&watches, &target, &start_op("/w")).await;
    let response = run(&watches, &target, &poll_op("w-1", 0, None)).await;
    assert_eq!(failed_kind(&response), Some(FileOpErrorKind::UnknownWatch));
    assert_eq!(watches.count("l1"), 0);
    Ok(())
}

#[tokio::test]
async fn an_envd_failure_while_polling_is_reported() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/GetWatcherEvents"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let (watches, target) = watches_for(&server)?;
    run(&watches, &target, &start_op("/w")).await;
    let response = run(&watches, &target, &poll_op("w-1", 0, None)).await;
    assert_eq!(failed_kind(&response), Some(FileOpErrorKind::Io));
    assert_eq!(watches.count("l1"), 1);
    Ok(())
}

#[tokio::test]
async fn a_stop_tolerates_envd_forgetting_or_failing_to_remove_the_watcher() -> TestResult {
    for reply in [
        ResponseTemplate::new(404).set_body_json(json!({"code": "not_found", "message": "gone"})),
        ResponseTemplate::new(500).set_body_string("boom"),
    ] {
        let server = MockServer::start().await;
        mount_create(&server, "e1").await;
        mount_remove(&server, reply, 1).await;
        let (watches, target) = watches_for(&server)?;
        run(&watches, &target, &start_op("/w")).await;
        assert_eq!(
            run(&watches, &target, &stop_op("w-1")).await,
            FileOpResponse::WatchStopped {
                existed: FilePresence::Present
            }
        );
    }
    Ok(())
}

#[tokio::test]
async fn other_file_operations_are_unsupported() -> TestResult {
    let server = MockServer::start().await;
    let (watches, target) = watches_for(&server)?;
    let response = run(&watches, &target, &FileOp::Stat { path: "/w".to_owned() }).await;
    assert!(matches!(response, FileOpResponse::Unsupported { .. }));
    Ok(())
}

#[tokio::test]
async fn a_watch_nobody_polled_expires_and_is_removed_from_envd() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    mount_remove(&server, removed(), 1).await;
    let (envd, target) = envd_for(&server)?;
    let watches = Watches::new(envd).with_idle_expiry(Duration::ZERO);
    run(&watches, &target, &start_op("/w")).await;
    assert_eq!(watches.count("l1"), 1);
    let response = run(&watches, &target, &poll_op("w-1", 0, None)).await;
    assert_eq!(failed_kind(&response), Some(FileOpErrorKind::UnknownWatch));
    assert_eq!(watches.count("l1"), 0);
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_removes_all_its_watchers() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    mount_remove(&server, removed(), 2).await;
    let (watches, target) = watches_for(&server)?;
    run(&watches, &target, &start_op("/a")).await;
    run(&watches, &target, &start_op("/b")).await;
    watches.release("l1", &target).await;
    assert_eq!(watches.count("l1"), 0);
    // A lease with no watches releases without touching envd.
    watches.release("l1", &target).await;
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_does_not_wait_forever_for_envd() -> TestResult {
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/RemoveWatcher"))
        .respond_with(removed().set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    let (envd, target) = envd_for(&server)?;
    let watches = Watches::new(envd).with_release_timeout(Duration::from_millis(50));
    run(&watches, &target, &start_op("/w")).await;
    let started = std::time::Instant::now();
    watches.release("l1", &target).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(watches.count("l1"), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_overflow_is_published_once_to_the_evidence_timeline() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let js = nats.jetstream().await;
    let flood: Vec<Value> = (0..=MAX_WATCHER_EVENTS_PER_FETCH)
        .map(|index| created(&format!("f{index}")))
        .collect();

    // Without a stream to take it, the publish fails; the poll still answers.
    let lossy = MockServer::start().await;
    mount_create(&lossy, "e1").await;
    mount_events(&lossy, json!({"events": flood}), Some(1)).await;
    mount_events(&lossy, json!({"events": []}), None).await;
    let (envd, target) = envd_for(&lossy)?;
    let watches = Watches::new(envd).with_poll_interval(POLL).with_evidence(js.clone());
    run(&watches, &target, &start_op("/w")).await;
    let (_, _, overflowed) = events_of(run(&watches, &target, &poll_op("w-1", 0, None)).await)?;
    assert_eq!(overflowed, FileWatchOverflow::Overflowed);

    js.create_stream(stream::Config {
        name: "EVIDENCE".to_owned(),
        subjects: vec!["sandbox.evidence.>".to_owned()],
        ..Default::default()
    })
    .await?;
    let flood: Vec<Value> = (0..=MAX_WATCHER_EVENTS_PER_FETCH)
        .map(|index| created(&format!("f{index}")))
        .collect();
    let server = MockServer::start().await;
    mount_create(&server, "e1").await;
    mount_events(&server, json!({"events": flood}), Some(1)).await;
    mount_events(&server, json!({"events": []}), None).await;
    let (envd, target) = envd_for(&server)?;
    let watches = Watches::new(envd).with_poll_interval(POLL).with_evidence(js.clone());
    run(&watches, &target, &start_op("/w")).await;
    let (events, _, overflowed) = events_of(run(&watches, &target, &poll_op("w-1", 0, None)).await)?;
    assert_eq!(overflowed, FileWatchOverflow::Overflowed);
    let last = events.last().map_or(0, |event| event.seq);
    // The loss is still visible to the caller but is published only once.
    run(&watches, &target, &poll_op("w-1", last, None)).await;
    // A poll of a watch that is gone has nothing to publish.
    run(&watches, &target, &poll_op("w-9", 0, None)).await;

    let mut stream = js.get_stream("EVIDENCE").await?;
    assert_eq!(stream.info().await?.state.messages, 1);
    let message = stream
        .get_last_raw_message_by_subject("sandbox.evidence.l1.file-watch-overflow")
        .await?;
    let evidence: Value = serde_json::from_slice(&message.payload)?;
    assert_eq!(evidence["watch_id"], "w-1");
    assert_eq!(evidence["dropped_through"], MAX_WATCHER_EVENTS_PER_FETCH);
    Ok(())
}

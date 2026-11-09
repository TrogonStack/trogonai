use std::future::{pending, ready};

use super::*;
use testcontainers_modules::testcontainers::bollard::API_DEFAULT_VERSION;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

enum Reply {
    Json(Value),
    Unavailable,
    Logs(Vec<u8>),
    Pending,
}

struct Exchange {
    path: String,
    reply: Reply,
}

// A protocol peer exercises Bollard's real HTTP, model and multiplexed-log
// decoding, without changing global Docker configuration or other fixtures.
struct DockerPeer {
    docker: Docker,
    task: JoinHandle<()>,
}

impl DockerPeer {
    async fn start(exchanges: Vec<Exchange>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for exchange in exchanges {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                    assert!(request.len() < 8192);
                }
                let request = String::from_utf8(request).unwrap();
                let target = request.split_ascii_whitespace().nth(1).unwrap();
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                assert_eq!(path, exchange.path);
                if exchange.path.ends_with("/logs") {
                    for option in ["stderr=true", "follow=false", "tail=128"] {
                        assert!(query.split('&').any(|part| part == option), "{query}");
                    }
                    assert!(!query.contains("stdout=true"));
                } else if exchange.path.starts_with("/containers/") {
                    assert!(query.split('&').any(|part| part == "size=true"));
                }
                let (status, content_type, body) = match exchange.reply {
                    Reply::Json(value) => (200, "application/json", serde_json::to_vec(&value).unwrap()),
                    Reply::Unavailable => (
                        500,
                        "application/json",
                        br#"{"message":"sensitive-fixture-marker"}"#.to_vec(),
                    ),
                    Reply::Logs(bytes) => (200, "application/vnd.docker.raw-stream", bytes),
                    Reply::Pending => pending().await,
                };
                socket.write_all(format!("HTTP/1.1 {status} response\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            }
        });
        Self {
            docker: Docker::connect_with_http(&format!("http://{address}"), 10, API_DEFAULT_VERSION).unwrap(),
            task,
        }
    }

    async fn finish(&mut self) {
        timeout(Duration::from_secs(5), &mut self.task).await.unwrap().unwrap();
    }
}

impl Drop for DockerPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn inspected(diagnostics: &FixtureDiagnostics) -> Value {
    json!({
        "Id": "a".repeat(64), "Image": format!("sha256:{}", "b".repeat(64)),
        "Name": format!("/{}", diagnostics.name),
        "Config": {"Image": diagnostics.image, "Labels": {constants::LABEL: diagnostics.name}},
        "State": {"Status": "exited", "Running": false, "OOMKilled": true, "ExitCode": 137, "Error": "permission denied"},
        "SizeRw": 321, "SizeRootFs": 654,
    })
}

fn inspect(diagnostics: &FixtureDiagnostics, reply: Reply) -> Exchange {
    Exchange {
        path: format!("/containers/{}/json", diagnostics.name),
        reply,
    }
}

fn logs(reply: Reply) -> Exchange {
    Exchange {
        path: format!("/containers/{}/logs", "a".repeat(64)),
        reply,
    }
}

fn image(reply: Reply) -> Exchange {
    Exchange {
        path: format!("/images/sha256:{}/json", "b".repeat(64)),
        reply,
    }
}

fn frame(channel: u8, bytes: &[u8]) -> Vec<u8> {
    let mut frame = vec![channel, 0, 0, 0];
    frame.extend(u32::try_from(bytes.len()).unwrap().to_be_bytes());
    frame.extend(bytes);
    frame
}

fn observations(report: &str) -> Vec<Value> {
    report
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["observation"].clone())
        .collect()
}

#[tokio::test]
async fn a_snapshot_before_creation_reports_unavailability_without_searching_other_containers() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    diagnostics.snapshot(docker_client_instance()).await;
    let report = diagnostics.report();
    assert_eq!(
        observations(&report),
        vec![json!({"kind":"collection", "operation":"container_inspect", "outcome":"unavailable"})]
    );
    assert!(!report.contains("owned-container="));
}

#[tokio::test]
async fn docker_initialization_failure_preserves_the_owned_handle_and_suppresses_error_detail() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    let unavailable = || {
        ready(Err(ClientError::InvalidDockerHost(
            "sensitive-fixture-marker".to_owned(),
        )))
    };
    diagnostics.snapshot(unavailable()).await;
    let id = "a".repeat(64);
    let report = diagnostics.capture_owned(&id, &unavailable).await;
    assert!(report.contains(&format!("owned-container={id}")));
    let records = observations(&report);
    assert_eq!(
        records
            .iter()
            .filter(|value| value["operation"] == "docker_client" && value["outcome"] == "unavailable")
            .count(),
        2
    );
    assert!(
        records
            .iter()
            .any(|value| value["kind"] == "container_handle" && value["container_id"] == id)
    );
    assert!(!report.contains("sensitive-fixture-marker"));
}

#[tokio::test]
async fn ownership_checks_fail_closed_before_any_log_or_image_read() {
    for field in ["name", "label", "image"] {
        let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
        let mut response = inspected(&diagnostics);
        match field {
            "name" => response["Name"] = json!("/sensitive-fixture-marker"),
            "label" => response["Config"]["Labels"][constants::LABEL] = json!("sensitive-fixture-marker"),
            "image" => response["Config"]["Image"] = json!("sensitive-fixture-marker"),
            _ => unreachable!(),
        }
        let mut peer = DockerPeer::start(vec![inspect(&diagnostics, Reply::Json(response))]).await;
        diagnostics.snapshot(ready(Ok(peer.docker.clone()))).await;
        peer.finish().await;
        let report = diagnostics.report();
        assert_eq!(
            observations(&report),
            vec![json!({"kind":"collection", "operation":"container_inspect", "outcome":"ownership_mismatch"})]
        );
        assert!(!report.contains("owned-container="));
        assert!(!report.contains("sensitive-fixture-marker"));
    }
}

#[tokio::test]
async fn invalid_snapshot_ids_are_never_used_as_log_or_image_targets() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    let mut response = inspected(&diagnostics);
    response["Id"] = json!("sensitive-fixture-marker");
    response["Image"] = json!("sha256:sensitive-fixture-marker");
    let mut peer = DockerPeer::start(vec![inspect(&diagnostics, Reply::Json(response))]).await;
    diagnostics.snapshot(ready(Ok(peer.docker.clone()))).await;
    peer.finish().await;
    let report = diagnostics.report();
    let records = observations(&report);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["container_id"], Value::Null);
    assert_eq!(records[0]["image_id"], Value::Null);
    assert_eq!(records[0]["state_error_selected"], "permission denied");
    assert!(!report.contains("sensitive-fixture-marker"));
}

#[tokio::test]
async fn image_inspect_failure_keeps_selected_container_state() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    let mut peer = DockerPeer::start(vec![
        inspect(&diagnostics, Reply::Json(inspected(&diagnostics))),
        logs(Reply::Logs(Vec::new())),
        image(Reply::Unavailable),
    ])
    .await;
    diagnostics.snapshot(ready(Ok(peer.docker.clone()))).await;
    peer.finish().await;
    let report = diagnostics.report();
    let records = observations(&report);
    assert_eq!(records[0]["oom_killed"], true);
    assert_eq!(records[0]["exit_code"], 137);
    assert_eq!(records[0]["size_root_fs"], 654);
    assert_eq!(
        records[1],
        json!({"kind":"collection", "operation":"image_inspect", "outcome":"unavailable"})
    );
    assert!(!report.contains("sensitive-fixture-marker"));
}

#[tokio::test]
async fn image_metadata_and_repository_digests_are_independently_allowlisted() {
    for (architecture, os) in [
        ("arm64", "linux"),
        ("sensitive-fixture-marker", "sensitive-fixture-marker"),
    ] {
        let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
        let hash = format!("sha256:{}", "c".repeat(64));
        let mut peer = DockerPeer::start(vec![
            inspect(&diagnostics, Reply::Json(inspected(&diagnostics))),
            logs(Reply::Logs(Vec::new())),
            image(Reply::Json(
                json!({"Architecture": architecture, "Os": os, "RepoDigests": [
                    "sensitive-fixture-marker", format!("private/sensitive-fixture-marker@{hash}"),
                    "nats@sha256:invalid", format!("nats@{hash}"), format!("docker.io/library/nats@{hash}"),
                    format!("nats@sha256:{}", "d".repeat(64)),
                ]}),
            )),
        ])
        .await;
        diagnostics.snapshot(ready(Ok(peer.docker.clone()))).await;
        peer.finish().await;
        let report = diagnostics.report();
        let records = observations(&report);
        assert_eq!(records[1]["repo_digests"], json!([hash, hash]));
        if architecture == "arm64" {
            assert_eq!(records[1]["architecture"], "arm64");
            assert_eq!(records[1]["os"], "linux");
        } else {
            assert_eq!(records[1]["architecture"], Value::Null);
            assert_eq!(records[1]["os"], Value::Null);
        }
        assert!(!report.contains("sensitive-fixture-marker"));
        assert!(!report.contains(&"d".repeat(64)));
    }
}

#[tokio::test]
async fn failed_stderr_tail_does_not_prevent_a_later_snapshot() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    let mut peer = DockerPeer::start(vec![
        logs(Reply::Unavailable),
        inspect(&diagnostics, Reply::Unavailable),
    ])
    .await;
    let report = diagnostics
        .capture_owned(&"a".repeat(64), &|| ready(Ok(peer.docker.clone())))
        .await;
    peer.finish().await;
    let records = observations(&report);
    assert_eq!(
        records[1],
        json!({"kind":"collection", "operation":"stderr_tail", "outcome":"unavailable"})
    );
    assert_eq!(
        records[2],
        json!({"kind":"collection", "operation":"container_inspect", "outcome":"unavailable"})
    );
    assert!(!report.contains("sensitive-fixture-marker"));
}

#[tokio::test]
async fn a_finite_tail_reads_at_most_128_frames_and_ignores_stdout() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    let mut bytes = frame(2, b"[1] 2026/11/09 [INF] Server is ready\n");
    for _ in 1..128 {
        bytes.extend(frame(1, b"[1] 2026/11/09 [INF] Starting nats-server\n"));
    }
    bytes.extend(frame(2, b"[1] 2026/11/09 [INF] Starting JetStream\n"));
    let mut peer = DockerPeer::start(vec![logs(Reply::Logs(bytes))]).await;
    diagnostics.logs(&peer.docker, &"a".repeat(64)).await;
    peer.finish().await;
    let report = diagnostics.report();
    assert!(report.contains("Server is ready"));
    assert!(!report.contains("Starting nats-server"));
    assert!(!report.contains("Starting JetStream"));
}

#[tokio::test]
async fn a_finite_tail_stops_at_64_kib_independently_of_frame_count() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    let mut bytes = frame(2, &vec![b'x'; constants::MAX_INPUT + 1]);
    bytes.extend(frame(2, b"[1] 2026/11/09 [INF] Server is ready\n"));
    let mut peer = DockerPeer::start(vec![logs(Reply::Logs(bytes))]).await;
    diagnostics.logs(&peer.docker, &"a".repeat(64)).await;
    peer.finish().await;
    let report = diagnostics.report();
    assert!(report.contains("diagnostics_truncated=true"));
    assert!(!report.contains("Server is ready"));
}

#[tokio::test]
async fn stalled_failure_tail_is_cancelled_at_the_original_collection_budget() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    let peer = DockerPeer::start(vec![logs(Reply::Pending)]).await;
    let started = Instant::now();
    let report = diagnostics
        .capture_owned(&"a".repeat(64), &|| ready(Ok(peer.docker.clone())))
        .await;
    assert!(started.elapsed() >= constants::COLLECTION_BUDGET);
    assert!(started.elapsed() < constants::COLLECTION_BUDGET + Duration::from_secs(1));
    let records = observations(&report);
    assert_eq!(records[0]["kind"], "container_handle");
    assert_eq!(
        records[1],
        json!({"kind":"collection", "operation":"failure_collection", "outcome":"collection_timeout"})
    );
}

#[tokio::test(start_paused = true)]
async fn observer_uses_request_relative_times_and_a_finite_budget_without_completing() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    let calls = std::cell::RefCell::new(Vec::new());
    let connect = || {
        calls.borrow_mut().push(diagnostics.started.elapsed());
        pending::<Result<Docker, ClientError>>()
    };
    let observer = diagnostics.observe_startup(&connect);
    tokio::pin!(observer);
    for seconds in [1, 30, 55] {
        tokio::time::advance(diagnostics.started + Duration::from_secs(seconds) - Instant::now()).await;
        assert!(futures::poll!(&mut observer).is_pending());
        tokio::time::advance(constants::COLLECTION_BUDGET).await;
        assert!(futures::poll!(&mut observer).is_pending());
    }
    assert_eq!(
        *calls.borrow(),
        vec![Duration::from_secs(1), Duration::from_secs(30), Duration::from_secs(55)]
    );
    assert_eq!(
        observations(&diagnostics.report()),
        vec![json!({"kind":"collection", "operation":"startup_snapshot", "outcome":"collection_timeout"}); 3]
    );
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!(futures::poll!(&mut observer).is_pending());
    assert_eq!(calls.borrow().len(), 3);
}

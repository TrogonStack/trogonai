mod common;
mod nats_common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_nats::HeaderMap;
use async_trait::async_trait;
use bytes::Bytes;
use common::{TestResult, start, start_with};
use futures::StreamExt;
use serde_json::{Value, json};
use trogon_sandbox_contract::{CommandOutput, LeaseId, PollCommandResponse, RunCommandResponse};
use trogon_sandbox_memory_worker::{CommandCtx, CommandHandler, CommandTable, FrameSink};
use trogon_sandbox_protocol::subjects::file_watch_overflow_evidence;

#[derive(Default)]
struct Counts {
    started: AtomicUsize,
    stopped: AtomicUsize,
}

struct Stopped(Arc<Counts>);

impl Drop for Stopped {
    fn drop(&mut self) {
        self.0.stopped.fetch_add(1, Ordering::SeqCst);
    }
}

struct Held(Arc<Counts>);

#[async_trait]
impl CommandHandler for Held {
    async fn run(&self, context: CommandCtx) -> CommandOutput {
        let _stopped = Stopped(Arc::clone(&self.0));
        self.0.started.fetch_add(1, Ordering::SeqCst);
        context.cancel.cancelled().await;
        CommandOutput::default()
    }
}

struct Crashing;

#[async_trait]
impl CommandHandler for Crashing {
    async fn run(&self, _context: CommandCtx) -> CommandOutput {
        let empty: Vec<u8> = Vec::new();
        CommandOutput {
            exit_code: i32::from(empty[std::hint::black_box(0)]),
            ..Default::default()
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn named_launches_share_one_execution_and_shutdown_ends_it() -> TestResult {
    let counts = Arc::new(Counts::default());
    let mut commands = CommandTable::empty();
    commands.register("held", Held(Arc::clone(&counts)));
    let h = start_with(commands).await?;
    h.grant("named").await?;
    let launch = |key: &str| json!({"lease_id": "named", "command": ["held"], "detached": true, "process": {"name": "web"}, "launch_key": key});
    let (first, second) = tokio::join!(
        h.call::<RunCommandResponse>("run_command", "named", launch("creator")),
        h.call::<RunCommandResponse>("run_command", "named", launch("joiner")),
    );
    let (RunCommandResponse::Started(first), RunCommandResponse::Started(second)) = (first?, second?) else {
        return Err("both launches must start".into());
    };
    assert_eq!(first.execution_id, second.execution_id);
    let mut origins = [format!("{:?}", first.origin), format!("{:?}", second.origin)];
    origins.sort();
    assert_eq!(origins, ["Created".to_string(), "Reused".to_string()]);
    while counts.started.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(counts.started.load(Ordering::SeqCst), 1);
    h.worker.shutdown().await;
    assert_eq!(counts.stopped.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn unrouted_messages_are_ignored_and_bad_ones_are_survived() -> TestResult {
    let h = start().await?;
    h.grant("l1").await?;

    h.client
        .publish("sandbox.provider.mock.lease", Bytes::from_static(b"{}"))
        .await?;
    h.client
        .publish("sandbox.provider.mock.run_command.l1", Bytes::from_static(b"{}"))
        .await?;
    h.client
        .publish("sandbox.provider.mock.unknown_verb.l1", Bytes::from_static(b"{}"))
        .await?;
    h.client
        .publish("sandbox.provider.mock..l1", Bytes::from_static(b"{}"))
        .await?;
    let silent = tokio::time::timeout(
        Duration::from_millis(300),
        h.client
            .request("sandbox.provider.mock.read_file.not-held", Bytes::from_static(b"{}")),
    )
    .await;
    assert!(silent.is_err() || silent.is_ok_and(|reply| reply.is_err()));

    let mut headers = HeaderMap::new();
    headers.insert("Sandbox-Transfer", "bogus");
    let bogus = tokio::time::timeout(
        Duration::from_millis(500),
        h.client
            .request_with_headers("sandbox.provider.mock.read_file.l1", headers, Bytes::from_static(b"{}")),
    )
    .await;
    assert!(bogus.is_err() || bogus.is_ok_and(|reply| reply.is_err()));

    let empty_id = h.dispatch_with("", common::lease_request()).await?;
    assert!(empty_id.get("decode_error").is_some());

    // The worker still serves after all of that.
    let read = h
        .call_raw("read_file", "l1", json!({"lease_id": "l1", "path": "/x"}))
        .await?;
    assert!(read.to_string().contains("not_found"));

    h.client
        .publish("sandbox.provider.mock.cancel.l1", Bytes::new())
        .await?;
    while h.worker.lease_count() != 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn signals_and_crashes_end_commands_with_their_status() -> TestResult {
    let mut commands = CommandTable::default();
    commands.register("crash", Crashing);
    let h = start_with(commands).await?;
    h.grant("l1").await?;
    for (signal, number) in [("hup", 1), ("int", 2), ("quit", 3), ("usr1", 10), ("usr2", 12)] {
        let RunCommandResponse::Started(started) = h
            .call(
                "run_command",
                "l1",
                json!({"lease_id": "l1", "command": ["sleep", "60"], "detached": true}),
            )
            .await?
        else {
            return Err("not started".into());
        };
        let target = json!({"lease_id": "l1", "execution_id": started.execution_id});
        let mut kill = target.clone();
        kill["signal"] = json!(signal);
        h.call_raw("kill_command", "l1", kill).await?;
        loop {
            if let PollCommandResponse::Completed(output) = h.call("poll_command", "l1", target.clone()).await? {
                assert_eq!(output.exit_code, 128 + number);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    let RunCommandResponse::Started(crashed) = h
        .call(
            "run_command",
            "l1",
            json!({"lease_id": "l1", "command": ["crash"], "detached": true}),
        )
        .await?
    else {
        return Err("not started".into());
    };
    let reply = h
        .call_raw(
            "kill_command",
            "l1",
            json!({"lease_id": "l1", "execution_id": crashed.execution_id}),
        )
        .await?;
    assert!(!reply.is_null());
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn readiness_polls_until_its_deadline() -> TestResult {
    let h = start().await?;
    h.grant("l1").await?;
    let RunCommandResponse::Started(started) = h
        .call(
            "run_command",
            "l1",
            json!({"lease_id": "l1", "command": ["serve"], "detached": true,
                   "process": {"readiness": {"kind": "output_contains", "needle": "never printed"}}}),
        )
        .await?
    else {
        return Err("not started".into());
    };
    let reply = h
        .call_raw(
            "wait_ready",
            "l1",
            json!({"lease_id": "l1", "execution_id": started.execution_id, "timeout_seconds": 1}),
        )
        .await?;
    assert!(reply.to_string().contains("not_ready"));
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn watches_report_quiet_polls_and_overflow_evidence() -> TestResult {
    let h = start().await?;
    h.grant("l1").await?;
    let op = |op: Value| json!({"lease_id": "l1", "op": op});
    let watch = h
        .call_raw(
            "file_op",
            "l1",
            op(json!({"op": "watch_start", "path": "/", "recursive": true})),
        )
        .await?;
    let id = watch["watch_id"].clone();
    let quiet = h
        .call_raw(
            "file_op",
            "l1",
            op(json!({"op": "watch_poll", "watch_id": id, "after_seq": 0, "wait_ms": 100})),
        )
        .await?;
    assert!(!quiet.is_null());

    let mut evidence = h
        .client
        .subscribe(file_watch_overflow_evidence(&LeaseId::new("l1")))
        .await?;
    h.client.flush().await?;
    let files: Vec<Value> = (0..4200)
        .map(|n| json!({"path": format!("/w/{n}"), "content": [1]}))
        .collect();
    h.call_raw("write_files", "l1", json!({"lease_id": "l1", "files": files}))
        .await?;
    let poll = h
        .call_raw(
            "file_op",
            "l1",
            op(json!({"op": "watch_poll", "watch_id": id, "after_seq": 0, "wait_ms": 1000})),
        )
        .await?;
    assert!(poll.to_string().contains("overflowed"), "{poll}");
    let message = tokio::time::timeout(Duration::from_secs(10), evidence.next())
        .await?
        .ok_or("closed")?;
    assert!(!message.payload.is_empty());
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn the_worker_exposes_its_authority_surface() -> TestResult {
    let h = start().await?;
    h.grant("l1").await?;
    h.grant("l2").await?;
    let held = h.worker.authority_held_leases();
    assert_eq!(held.len(), 2);
    assert_eq!(h.worker.lease_count(), 2);
    let heartbeat = h.worker.heartbeat_source(10, vec!["eu-1".to_string()])();
    assert_eq!(heartbeat.capacity.current_leases, 2);
    assert_eq!(heartbeat.profile.placement.regions, vec!["eu-1".to_string()]);
    for lease in &held {
        assert_eq!(
            h.worker.authority_freeze(lease).await,
            trogon_sandbox_protocol::FreezeOutcome::Unsupported
        );
    }
    for lease in &held {
        assert_eq!(
            h.worker.authority_purge(lease).await,
            trogon_sandbox_protocol::PurgeOutcome::Purged
        );
    }
    assert_eq!(h.worker.lease_count(), 0);
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn publishing_over_the_server_limit_is_logged_not_fatal() -> TestResult {
    let h = start().await?;
    FrameSink::publish(
        &h.client,
        "terminal.test".to_string(),
        Bytes::from(vec![0u8; 2 * 1024 * 1024]),
    )
    .await;
    FrameSink::publish(&h.client, "terminal.test".to_string(), Bytes::from_static(b"ok")).await;
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_and_concurrent_grants_keep_one_lease() -> TestResult {
    let h = start().await?;
    let (first, second) = tokio::join!(h.grant("same"), h.grant("same"));
    first?;
    second?;
    h.grant("same").await?;
    assert_eq!(h.worker.lease_count(), 1);
    h.worker.shutdown().await;
    Ok(())
}

async fn write_marker(h: &common::Harness, lease: &str) -> TestResult {
    let write = h
        .call_raw(
            "write_files",
            lease,
            json!({"lease_id": lease, "files": [{"path": "marker.txt", "content": [1, 2, 3]}]}),
        )
        .await?;
    assert!(write.to_string().contains("written"), "{write}");
    Ok(())
}

async fn read_marker(h: &common::Harness, lease: &str) -> Result<Value, Box<dyn std::error::Error>> {
    h.call_raw("read_file", lease, json!({"lease_id": lease, "path": "/marker.txt"}))
        .await
}

#[tokio::test]
async fn a_repeated_dispatch_keeps_the_lease_and_what_it_holds() -> TestResult {
    let h = start().await?;
    let first = h.dispatch_with("kept", common::lease_request()).await?;
    write_marker(&h, "kept").await?;

    let second = h.dispatch_with("kept", common::lease_request()).await?;

    assert_eq!(first, second);
    let read = read_marker(&h, "kept").await?;
    assert!(read.to_string().contains("content"), "{read}");
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_cancelled_lease_id_comes_back_as_an_empty_lease() -> TestResult {
    let h = start().await?;
    h.grant("reused").await?;
    write_marker(&h, "reused").await?;
    h.client
        .publish("sandbox.provider.mock.cancel.reused", Bytes::new())
        .await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while h.worker.lease_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;

    h.grant("reused").await?;

    assert_eq!(h.worker.lease_count(), 1);
    let read = read_marker(&h, "reused").await?;
    assert!(read.to_string().contains("not_found"), "{read}");
    h.worker.shutdown().await;
    Ok(())
}

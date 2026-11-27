//! Change tracking and file watching against a real container.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker file_tracking
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_lease_id, lease_request};

use futures::StreamExt as _;
use trogon_sandbox_contract::{
    BootMode, CommandEnv, FileContentHashing, FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, FileRecursion,
    FileWatchEventKind, LeaseId, LeaseRequest, StreamedOutput,
};
use trogon_sandbox_docker_worker::adapter::DockerWorkerProvider;
use trogon_sandbox_protocol::Provider as _;

async fn file_op(provider: &DockerWorkerProvider, lease: &LeaseId, op: FileOp) -> TestResult<FileOpResponse> {
    provider
        .file_op_for_lease(&FileOpRequest {
            lease_id: lease.clone(),
            op,
            as_user: None,
        })
        .await
        .ok_or_else(|| "the lease has no running container".into())
}

async fn sh(provider: &DockerWorkerProvider, lease: &LeaseId, script: &str) -> TestResult {
    let out = provider
        .run_command_for_lease(
            lease,
            vec!["sh".into(), "-c".into(), script.into()],
            None,
            &CommandEnv::default(),
            30,
            None,
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await?;
    assert_eq!(out.exit_code, 0, "{script}: {}", String::from_utf8_lossy(&out.stderr));
    Ok(())
}

fn bare_lease_request() -> TestResult<LeaseRequest> {
    lease_request("file-tracking-test", BootMode::Bare)
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_changes_and_watch_follow_container_files() -> TestResult {
    let provider = build_provider("file_tracking").await?;
    let lease = fresh_lease_id();
    provider.lease(&bare_lease_request()?, &lease).await?;

    sh(
        &provider,
        &lease,
        "mkdir -p /work/sub && echo one > /work/a.txt && echo two > /work/sub/b.txt",
    )
    .await?;

    let FileOpResponse::Checkpointed { checkpoint_id, entries } = file_op(
        &provider,
        &lease,
        FileOp::Checkpoint {
            path: "/work".into(),
            hash: FileContentHashing::default(),
        },
    )
    .await?
    else {
        return Err("expected a checkpoint".into());
    };
    assert_eq!(entries, 3_u64, "a.txt, sub and sub/b.txt");

    let FileOpResponse::WatchStarted { watch_id } = file_op(
        &provider,
        &lease,
        FileOp::WatchStart {
            path: "/work".into(),
            recursive: FileRecursion::Recursive,
        },
    )
    .await?
    else {
        return Err("expected a watch".into());
    };

    sh(
        &provider,
        &lease,
        "echo three > /work/new.txt && echo changed-longer > /work/a.txt && rm /work/sub/b.txt",
    )
    .await?;

    let changes = file_op(
        &provider,
        &lease,
        FileOp::Changes {
            since: Some(checkpoint_id),
            path: None,
        },
    )
    .await?;
    let FileOpResponse::Changes {
        added,
        modified,
        deleted,
        ..
    } = changes
    else {
        return Err(format!("expected changes, got {changes:?}").into());
    };
    assert_eq!(added, vec!["/work/new.txt"]);
    assert_eq!(modified, vec!["/work/a.txt"]);
    assert_eq!(deleted, vec!["/work/sub/b.txt"]);

    let FileOpResponse::WatchEvents { events, .. } = file_op(
        &provider,
        &lease,
        FileOp::WatchPoll {
            watch_id,
            after_seq: 0,
            wait_ms: Some(5000),
        },
    )
    .await?
    else {
        return Err("expected events".into());
    };
    let seen: Vec<_> = events.iter().map(|e| (e.kind, e.path.as_str())).collect();
    assert!(
        seen.contains(&(FileWatchEventKind::Created, "/work/new.txt")),
        "{seen:?}"
    );
    assert!(
        seen.contains(&(FileWatchEventKind::Modified, "/work/a.txt")),
        "{seen:?}"
    );
    assert!(
        seen.contains(&(FileWatchEventKind::Deleted, "/work/sub/b.txt")),
        "{seen:?}"
    );

    // This image has no /workspace, so no start baseline was recorded and the container's own
    // diff against its image answers.
    let since_start = file_op(
        &provider,
        &lease,
        FileOp::Changes {
            since: None,
            path: Some("/work".into()),
        },
    )
    .await?;
    let FileOpResponse::Changes { since, added, .. } = since_start else {
        return Err(format!("expected changes, got {since_start:?}").into());
    };
    assert_eq!(since, "start");
    assert!(added.contains(&"/work/new.txt".to_string()), "{added:?}");

    // A name from another lease is unknown here.
    let other = fresh_lease_id();
    provider.lease(&bare_lease_request()?, &other).await?;
    let foreign = file_op(
        &provider,
        &other,
        FileOp::Changes {
            since: Some("cp-1".into()),
            path: None,
        },
    )
    .await?;
    assert!(matches!(
        foreign,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::UnknownCheckpoint,
            ..
        }
    ));

    provider.cancel(&lease).await?;
    provider.cancel(&other).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_overflow_lands_on_the_lease_evidence_subject() -> TestResult {
    let provider = build_provider("file_tracking").await?;
    let nats = async_nats::connect(provider.server.address()).await?;
    let lease = fresh_lease_id();
    let mut evidence = nats
        .subscribe(trogon_sandbox_protocol::subjects::file_watch_overflow_evidence(&lease))
        .await?;
    nats.flush().await?;
    provider.lease(&bare_lease_request()?, &lease).await?;
    sh(&provider, &lease, "mkdir -p /work").await?;

    let FileOpResponse::WatchStarted { watch_id } = file_op(
        &provider,
        &lease,
        FileOp::WatchStart {
            path: "/work".into(),
            recursive: FileRecursion::Recursive,
        },
    )
    .await?
    else {
        return Err("expected a watch".into());
    };

    // More events than a watch buffers, so the oldest are dropped.
    sh(
        &provider,
        &lease,
        "cd /work && i=0; while [ $i -lt 5000 ]; do : > f$i; i=$((i+1)); done",
    )
    .await?;

    let mut overflowed = false;
    for _ in 0..20 {
        let response = file_op(
            &provider,
            &lease,
            FileOp::WatchPoll {
                watch_id: watch_id.clone(),
                after_seq: 0,
                wait_ms: Some(500),
            },
        )
        .await?;
        let FileOpResponse::WatchEvents { overflowed: now, .. } = response else {
            return Err(format!("expected events, got {response:?}").into());
        };
        if now.overflowed() {
            overflowed = true;
            break;
        }
    }
    assert!(overflowed, "the poll reports the overflow");

    let message = tokio::time::timeout(std::time::Duration::from_secs(5), evidence.next())
        .await?
        .ok_or("the evidence subject closed")?;
    let published: trogon_sandbox_contract::FileWatchOverflowEvidence = serde_json::from_slice(&message.payload)?;
    assert_eq!(published.watch_id, watch_id);
    assert!(published.dropped_through > 0);

    provider.cancel(&lease).await?;
    Ok(())
}

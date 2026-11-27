mod common;
mod file_support;

use common::{TestResult, lease_id};
use file_support::{client_for, client_scripted, sent_op};
use futures::StreamExt;
use trogon_sandbox_client::{ClientError, FileWatchUpdate};
use trogon_sandbox_contract::{
    FileContentHashing, FileOp, FileOpErrorKind, FileOpResponse, FilePresence, FileRecursion, FileResultTruncation,
    FileWatchEvent, FileWatchEventKind, FileWatchOverflow, FileWatchTarget,
};

fn event(seq: u64, path: &str) -> FileWatchEvent {
    FileWatchEvent {
        seq,
        kind: FileWatchEventKind::Created,
        path: path.into(),
        is_dir: FileWatchTarget::File,
    }
}

#[tokio::test]
async fn file_changes_sends_the_baseline_and_returns_the_typed_lists() -> TestResult {
    let (client, log) = client_for(&FileOpResponse::Changes {
        since: "cp-1".into(),
        added: vec!["/w/a".into()],
        modified: vec![],
        deleted: vec!["/w/b".into()],
        truncated: FileResultTruncation::Complete,
    })?;
    let changes = client.file_changes(&lease_id(), Some("cp-1".into()), None).await?;
    assert_eq!(changes.since, "cp-1");
    assert_eq!(changes.added, vec!["/w/a"]);
    assert_eq!(changes.deleted, vec!["/w/b"]);
    assert!(changes.modified.is_empty());
    assert_eq!(changes.truncated, FileResultTruncation::Complete);
    assert_eq!(log.only()?.subject, "sandbox.lease.file_op.lease-1");
    assert_eq!(
        sent_op(&log)?,
        FileOp::Changes {
            since: Some("cp-1".into()),
            path: None
        }
    );
    Ok(())
}

#[tokio::test]
async fn file_checkpoint_returns_the_id_and_an_unknown_checkpoint_error_is_typed() -> TestResult {
    let (client, log) = client_for(&FileOpResponse::Checkpointed {
        checkpoint_id: "cp-1".into(),
        entries: 3,
    })?;
    let (id, entries) = client
        .file_checkpoint(&lease_id(), "/w", FileContentHashing::Content)
        .await?;
    assert_eq!((id.as_str(), entries), ("cp-1", 3));
    assert_eq!(
        sent_op(&log)?,
        FileOp::Checkpoint {
            path: "/w".into(),
            hash: FileContentHashing::Content
        }
    );

    let (client, _) = client_for(&FileOpResponse::Failed {
        kind: FileOpErrorKind::UnknownCheckpoint,
        message: "no such checkpoint".into(),
    })?;
    let error = client.file_changes(&lease_id(), Some("cp-9".into()), None).await.err();
    assert!(matches!(
        error,
        Some(ClientError::FileOpFailed {
            kind: FileOpErrorKind::UnknownCheckpoint,
            ..
        })
    ));
    Ok(())
}

#[tokio::test]
async fn file_watch_on_an_unsupported_provider_is_a_typed_error() -> TestResult {
    let (client, _) = client_for(&FileOpResponse::Unsupported {
        message: "no watch".into(),
    })?;
    let error = client
        .file_watch_start(&lease_id(), "/w", FileRecursion::Recursive)
        .await
        .err();
    assert!(matches!(error, Some(ClientError::FileOpUnsupported(_))));
    Ok(())
}

#[tokio::test]
async fn watch_verbs_send_their_operations_and_return_typed_replies() -> TestResult {
    let (client, log) = client_scripted(&[
        FileOpResponse::WatchStarted { watch_id: "w1".into() },
        FileOpResponse::WatchEvents {
            events: vec![event(1, "/w/a")],
            next_seq: 1,
            overflowed: FileWatchOverflow::Intact,
        },
        FileOpResponse::WatchStopped {
            existed: FilePresence::Present,
        },
    ])?;
    let id = lease_id();
    assert_eq!(client.file_watch_start(&id, "/w", FileRecursion::Flat).await?, "w1");
    let (events, next_seq, overflowed) = client.file_watch_poll(&id, "w1", 0, Some(100)).await?;
    assert_eq!(
        (events, next_seq, overflowed),
        (vec![event(1, "/w/a")], 1, FileWatchOverflow::Intact)
    );
    assert_eq!(client.file_watch_stop(&id, "w1").await?, FilePresence::Present);
    let ops: Vec<FileOp> = log
        .all()
        .iter()
        .map(|sent| serde_json::from_slice::<trogon_sandbox_contract::FileOpRequest>(&sent.payload).map(|r| r.op))
        .collect::<Result<_, _>>()?;
    assert_eq!(
        ops,
        vec![
            FileOp::WatchStart {
                path: "/w".into(),
                recursive: FileRecursion::Flat
            },
            FileOp::WatchPoll {
                watch_id: "w1".into(),
                after_seq: 0,
                wait_ms: Some(100)
            },
            FileOp::WatchStop { watch_id: "w1".into() },
        ]
    );
    Ok(())
}

#[tokio::test]
async fn watch_verbs_reject_a_reply_of_the_wrong_kind() -> TestResult {
    let (client, _) = client_for(&FileOpResponse::Renamed)?;
    let id = lease_id();
    assert!(
        client
            .file_checkpoint(&id, "/", FileContentHashing::Metadata)
            .await
            .is_err()
    );
    assert!(client.file_changes(&id, None, None).await.is_err());
    assert!(client.file_watch_start(&id, "/", FileRecursion::Flat).await.is_err());
    assert!(client.file_watch_poll(&id, "w", 0, None).await.is_err());
    assert!(client.file_watch_stop(&id, "w").await.is_err());
    Ok(())
}

#[tokio::test]
async fn watch_files_yields_events_and_overflow_then_ends_on_an_error() -> TestResult {
    let (client, log) = client_scripted(&[
        FileOpResponse::WatchStarted { watch_id: "w1".into() },
        FileOpResponse::WatchEvents {
            events: vec![event(1, "/w/a"), event(2, "/w/b")],
            next_seq: 2,
            overflowed: FileWatchOverflow::Overflowed,
        },
        FileOpResponse::Failed {
            kind: FileOpErrorKind::Io,
            message: "gone".into(),
        },
        FileOpResponse::WatchStopped {
            existed: FilePresence::Absent,
        },
    ])?;
    let id = lease_id();
    let mut stream = client.watch_files(&id, "/w", FileRecursion::Recursive);
    assert_eq!(stream.next().await.transpose()?, Some(FileWatchUpdate::Overflowed));
    assert_eq!(
        stream.next().await.transpose()?,
        Some(FileWatchUpdate::Event(event(1, "/w/a")))
    );
    assert_eq!(
        stream.next().await.transpose()?,
        Some(FileWatchUpdate::Event(event(2, "/w/b")))
    );
    assert!(matches!(
        stream.next().await,
        Some(Err(ClientError::FileOpFailed { .. }))
    ));
    assert!(stream.next().await.is_none());
    // start, poll, failing poll, then the best-effort stop
    let sent = log.all();
    assert_eq!(sent.len(), 4);
    let poll: trogon_sandbox_contract::FileOpRequest = serde_json::from_slice(&sent[2].payload)?;
    assert_eq!(
        poll.op,
        FileOp::WatchPoll {
            watch_id: "w1".into(),
            after_seq: 2,
            wait_ms: Some(FileOp::WATCH_MAX_WAIT_MS)
        }
    );
    Ok(())
}

#[tokio::test]
async fn watch_files_ends_when_the_watch_cannot_start() -> TestResult {
    let (client, _) = client_for(&FileOpResponse::WatchEvents {
        events: vec![],
        next_seq: 0,
        overflowed: FileWatchOverflow::Intact,
    })?;
    let id = lease_id();
    let mut stream = client.watch_files(&id, "/w", FileRecursion::Recursive);
    assert!(matches!(stream.next().await, Some(Err(ClientError::Transport(_)))));
    assert!(stream.next().await.is_none());
    Ok(())
}

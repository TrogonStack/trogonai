//! Hashed change tracking and recursive file watches against a real E2B sandbox, including their survival across a
//! pause and resume of the exact owned sandbox, ending with proof that the sandbox is gone.
//!
//! Skipped unless `SANDBOX_E2B_HOSTED_TEST=1`; it reads `E2B_API_KEY` and optionally `SANDBOX_E2B_TEMPLATE`.

mod hosted_support;

use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;
use hosted_support::owned_lease::{OwnedLease, finish};
use hosted_support::{Boxed, GATE, TestResult};
use trogon_sandbox_contract::{
    FileContentHashing, FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, FilePresence, FileRecursion,
    FileWatchEvent, FileWatchEventKind, SandboxFreezeResponse,
};
use trogon_sandbox_e2b_worker::LeaseSandbox;

struct Files<'a> {
    probe: &'a OwnedLease,
    root: String,
}

impl Files<'_> {
    async fn file_op(&self, sandbox: &LeaseSandbox, op: FileOp) -> FileOpResponse {
        self.probe
            .provider
            .exec()
            .handle_file_op(
                sandbox,
                &FileOpRequest {
                    lease_id: self.probe.lease_id.clone(),
                    op,
                    as_user: None,
                },
            )
            .await
    }

    async fn checkpoint(&self, sandbox: &LeaseSandbox, path: &str) -> Result<String, Boxed> {
        let response = self
            .file_op(
                sandbox,
                FileOp::Checkpoint {
                    path: path.to_owned(),
                    hash: FileContentHashing::Content,
                },
            )
            .await;
        match response {
            FileOpResponse::Checkpointed {
                checkpoint_id,
                entries: 3,
            } => Ok(checkpoint_id),
            other => Err(format!("a hashed checkpoint did not retain its tree: {other:?}").into()),
        }
    }

    async fn expect_changes(
        &self,
        sandbox: &LeaseSandbox,
        checkpoint: &str,
        path: Option<String>,
        expected: [&[String]; 3],
    ) -> TestResult {
        let response = self
            .file_op(
                sandbox,
                FileOp::Changes {
                    since: Some(checkpoint.to_owned()),
                    path,
                },
            )
            .await;
        let FileOpResponse::Changes {
            since,
            added,
            modified,
            deleted,
            truncated,
        } = response
        else {
            return Err(format!("tracking did not report its owned checkpoint: {response:?}").into());
        };
        if since != checkpoint
            || added != expected[0]
            || modified != expected[1]
            || deleted != expected[2]
            || truncated.is_truncated()
        {
            return Err(format!(
                "the change report differs: added={added:?}, modified={modified:?}, deleted={deleted:?}"
            )
            .into());
        }
        Ok(())
    }

    async fn expect_events(
        &self,
        sandbox: &LeaseSandbox,
        watch: &str,
        cursor: &mut u64,
        expected: &[(FileWatchEventKind, String)],
    ) -> Result<Vec<FileWatchEvent>, Boxed> {
        let mut collected = Vec::new();
        let waited = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let response = self
                    .file_op(
                        sandbox,
                        FileOp::WatchPoll {
                            watch_id: watch.to_owned(),
                            after_seq: *cursor,
                            wait_ms: Some(1000),
                        },
                    )
                    .await;
                let FileOpResponse::WatchEvents {
                    events,
                    next_seq,
                    overflowed,
                } = response
                else {
                    return Err(format!("a watch did not return events: {response:?}").into());
                };
                if overflowed.overflowed() {
                    return Err("a bounded file sequence must not overflow".into());
                }
                let mut previous = *cursor;
                for event in &events {
                    if event.seq <= previous || !event.path.starts_with(&format!("{}/", self.root)) {
                        return Err(format!("a watch must expose monotonic owned absolute paths: {event:?}").into());
                    }
                    previous = event.seq;
                }
                if next_seq != previous {
                    return Err("the watch cursor must identify the last delivered event".into());
                }
                *cursor = next_seq;
                collected.extend(events);
                if expected
                    .iter()
                    .all(|(kind, path)| collected.iter().any(|event| event.kind == *kind && event.path == *path))
                {
                    return Ok::<(), Boxed>(());
                }
            }
        })
        .await;
        match waited {
            Ok(result) => result?,
            Err(_) => {
                return Err(format!(
                    "expected watch events did not arrive: expected={expected:?}, observed={collected:?}"
                )
                .into());
            }
        }
        Ok(collected)
    }

    async fn pause_and_resume(&self) -> Result<LeaseSandbox, Boxed> {
        let id = self.probe.sandbox_id.as_deref().ok_or("no owned sandbox")?;
        let paused = self
            .probe
            .provider
            .set_paused_for_lease(&self.probe.lease_id, true)
            .await;
        if !matches!(paused, SandboxFreezeResponse::Done) {
            return Err(format!("the owned sandbox must pause through the provider contract: {paused:?}").into());
        }
        if self.probe.api.get_sandbox(id).await?.state != "paused" {
            return Err("the exact sandbox must report paused".into());
        }
        let resumed = self
            .probe
            .provider
            .set_paused_for_lease(&self.probe.lease_id, false)
            .await;
        if !matches!(resumed, SandboxFreezeResponse::Done) {
            return Err(format!("the owned sandbox must resume through the provider contract: {resumed:?}").into());
        }
        let sandbox = self.probe.sandbox().await?;
        if self.probe.api.get_sandbox(id).await?.state != "running" {
            return Err("the same sandbox must report running after resume".into());
        }
        Ok(sandbox)
    }
}

async fn exercise(probe: &mut OwnedLease) -> TestResult {
    let sandbox = probe.lease(180).await?;
    let root = format!("/home/user/hosted-file-events-{}", uuid::Uuid::new_v4());
    let files = Files {
        probe,
        root: root.clone(),
    };
    let watch_root = format!("{root}/watch");
    let nested = format!("{watch_root}/nested");
    let tracked = format!("{root}/tracked");
    let modified = format!("{tracked}/modified.txt");
    let retained = format!("{tracked}/retained.txt");
    let deleted = format!("{tracked}/deleted.txt");
    let added = format!("{tracked}/added.txt");
    let stamp = format!("{root}/stamp");
    probe
        .succeeding(
            &sandbox,
            &format!(
                "set -eu\ncommand -v stat >/dev/null\ncommand -v sha256sum >/dev/null\nmkdir -p {nested} {tracked}\nprintf aaaa > {modified}\nprintf keep > {retained}\nprintf gone > {deleted}\ntouch -r {modified} {stamp}"
            ),
        )
        .await?;
    let checkpoint = files.checkpoint(&sandbox, &tracked).await?;
    files
        .expect_changes(&sandbox, &checkpoint, None, [&[], &[], &[]])
        .await?;
    let stat = format!("stat -c '%s|%y' {modified}");
    let metadata_before = probe.succeeding(&sandbox, &stat).await?.stdout;
    probe
        .succeeding(
            &sandbox,
            &format!(
                "set -eu\nprintf bbbb > {modified}\ntouch -r {stamp} {modified}\ntouch {retained}\nrm {deleted}\nprintf new > {added}"
            ),
        )
        .await?;
    if metadata_before != probe.succeeding(&sandbox, &stat).await?.stdout {
        return Err("the rewrite must preserve the actual file size and nanosecond mtime".into());
    }
    let (added_set, modified_set, deleted_set) = (
        std::slice::from_ref(&added),
        std::slice::from_ref(&modified),
        std::slice::from_ref(&deleted),
    );
    files
        .expect_changes(&sandbox, &checkpoint, None, [added_set, modified_set, deleted_set])
        .await?;
    files
        .expect_changes(&sandbox, &checkpoint, Some(modified.clone()), [&[], modified_set, &[]])
        .await?;

    let started = files
        .file_op(
            &sandbox,
            FileOp::WatchStart {
                path: watch_root,
                recursive: FileRecursion::Recursive,
            },
        )
        .await;
    let FileOpResponse::WatchStarted { watch_id } = started else {
        return Err(format!("a recursive watch did not start: {started:?}").into());
    };
    let original = format!("{nested}/original.txt");
    let renamed = format!("{nested}/renamed.txt");
    let mut cursor = 0;
    let mut all_events = Vec::new();
    probe
        .succeeding(&sandbox, &format!("printf first > {original}"))
        .await?;
    all_events.extend(
        files
            .expect_events(
                &sandbox,
                &watch_id,
                &mut cursor,
                &[(FileWatchEventKind::Created, original.clone())],
            )
            .await?,
    );
    probe
        .succeeding(&sandbox, &format!("printf second >> {original}"))
        .await?;
    all_events.extend(
        files
            .expect_events(
                &sandbox,
                &watch_id,
                &mut cursor,
                &[(FileWatchEventKind::Modified, original.clone())],
            )
            .await?,
    );
    probe.succeeding(&sandbox, &format!("mv {original} {renamed}")).await?;
    all_events.extend(
        files
            .expect_events(
                &sandbox,
                &watch_id,
                &mut cursor,
                &[
                    (FileWatchEventKind::Deleted, original),
                    (FileWatchEventKind::Created, renamed.clone()),
                ],
            )
            .await?,
    );

    let resumed = files.pause_and_resume().await?;
    files
        .expect_changes(&resumed, &checkpoint, None, [added_set, modified_set, deleted_set])
        .await?;
    probe.succeeding(&resumed, &format!("rm {renamed}")).await?;
    all_events.extend(
        files
            .expect_events(
                &resumed,
                &watch_id,
                &mut cursor,
                &[(FileWatchEventKind::Deleted, renamed)],
            )
            .await?,
    );
    let sequences: BTreeSet<u64> = all_events.iter().map(|event| event.seq).collect();
    if sequences.len() != all_events.len() {
        return Err("watch cursor continuation must not replay prior events".into());
    }
    let stopped = files
        .file_op(
            &resumed,
            FileOp::WatchStop {
                watch_id: watch_id.clone(),
            },
        )
        .await;
    if !matches!(&stopped, FileOpResponse::WatchStopped { existed } if *existed == FilePresence::Present) {
        return Err(format!("the owned watcher must be explicitly removed: {stopped:?}").into());
    }
    let forgotten = files
        .file_op(
            &resumed,
            FileOp::WatchPoll {
                watch_id,
                after_seq: cursor,
                wait_ms: Some(1),
            },
        )
        .await;
    assert!(
        matches!(
            forgotten,
            FileOpResponse::Failed {
                kind: FileOpErrorKind::UnknownWatch,
                ..
            }
        ),
        "a stopped watcher must not retain access: {forgotten:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "paid hosted E2B: set SANDBOX_E2B_HOSTED_TEST=1 with E2B_API_KEY and use --ignored; creates one sandbox with a 180s TTL"]
async fn hosted_file_watch_and_hashed_tracking_survive_pause_resume() -> TestResult {
    let mut probe = OwnedLease::new(GATE, "hosted-file-events")?;
    let outcome = AssertUnwindSafe(exercise(&mut probe)).catch_unwind().await;
    finish(&probe, outcome).await
}

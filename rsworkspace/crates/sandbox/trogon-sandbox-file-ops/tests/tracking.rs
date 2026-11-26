use std::error::Error;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use trogon_sandbox_contract::{
    FileContentHashing, FileOp, FileOpErrorKind, FileOpResponse, FilePresence, FileRecursion, FileResultTruncation,
    FileWatchEvent, FileWatchEventKind, FileWatchOverflow, FileWatchOverflowEvidence, FileWatchTarget, LeaseId,
};
use trogon_sandbox_file_ops::{
    ChangeHint, FileOpFailure, Manifest, ManifestDiff, ManifestEntry, ManifestSource, Tracking,
};

type TestResult = Result<(), Box<dyn Error>>;

fn file(size: u64) -> ManifestEntry {
    ManifestEntry::file(size, Some(size.to_string()), None)
}

fn tree(entries: &[(&str, u64)]) -> Manifest {
    entries
        .iter()
        .map(|(path, size)| (path.to_string(), file(*size)))
        .collect()
}

/// A manifest source whose tree the test edits between calls.
#[derive(Default)]
struct Fake {
    current: Mutex<Manifest>,
    hints: Mutex<Vec<ChangeHint>>,
    since_start: Mutex<Option<Result<ManifestDiff, FileOpFailure>>>,
    after_wait: Mutex<Option<Manifest>>,
    fail: AtomicBool,
    yield_once: AtomicBool,
}

impl Fake {
    fn with(entries: &[(&str, u64)]) -> Self {
        let fake = Self::default();
        fake.set(tree(entries));
        fake
    }

    fn set(&self, manifest: Manifest) {
        if let Ok(mut current) = self.current.lock() {
            *current = manifest;
        }
    }

    fn hint(&self, path: &str, kind: FileWatchEventKind, target: FileWatchTarget) {
        if let Ok(mut hints) = self.hints.lock() {
            hints.push(ChangeHint {
                path: path.to_string(),
                kind,
                target,
            });
        }
    }

    fn provider(&self, answer: Result<ManifestDiff, FileOpFailure>) {
        if let Ok(mut since_start) = self.since_start.lock() {
            *since_start = Some(answer);
        }
    }

    fn snapshot(&self) -> Manifest {
        self.current.lock().map(|current| current.clone()).unwrap_or_default()
    }
}

impl ManifestSource for Fake {
    async fn manifest(
        &self,
        path: &str,
        _recursion: FileRecursion,
        _hashing: FileContentHashing,
    ) -> Result<Manifest, FileOpFailure> {
        if self.yield_once.swap(false, Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(FileOpFailure::NotFound { path: path.to_string() });
        }
        Ok(self.snapshot())
    }

    async fn changes_since_start(&self) -> Option<Result<ManifestDiff, FileOpFailure>> {
        self.since_start.lock().ok().and_then(|answer| answer.clone())
    }

    fn scan_interval(&self) -> Duration {
        Duration::from_millis(1)
    }

    async fn wait_for_change(&self, _path: &str, _recursion: FileRecursion, _timeout: Duration) {
        let next = self.after_wait.lock().ok().and_then(|mut next| next.take());
        if let Some(next) = next {
            self.set(next);
        }
    }

    fn drain_hints(&self, _watch_id: &str) -> Vec<ChangeHint> {
        self.hints
            .lock()
            .map(|mut hints| std::mem::take(&mut *hints))
            .unwrap_or_default()
    }
}

/// A source that only answers manifests, so the trait defaults serve everything else.
struct Bare(Manifest);

impl ManifestSource for Bare {
    async fn manifest(&self, _: &str, _: FileRecursion, _: FileContentHashing) -> Result<Manifest, FileOpFailure> {
        Ok(self.0.clone())
    }
}

fn checkpoint(path: &str) -> FileOp {
    FileOp::Checkpoint {
        path: path.into(),
        hash: FileContentHashing::Metadata,
    }
}

fn changes(since: Option<&str>, path: Option<&str>) -> FileOp {
    FileOp::Changes {
        since: since.map(String::from),
        path: path.map(String::from),
    }
}

fn watch_start(path: &str) -> FileOp {
    FileOp::WatchStart {
        path: path.into(),
        recursive: FileRecursion::Recursive,
    }
}

fn poll(watch_id: &str, after_seq: u64, wait_ms: Option<u32>) -> FileOp {
    FileOp::WatchPoll {
        watch_id: watch_id.into(),
        after_seq,
        wait_ms,
    }
}

async fn started(tracking: &Tracking, source: &Fake, path: &str) -> Result<String, Box<dyn Error>> {
    match tracking.serve("lease", &watch_start(path), source).await {
        Some(FileOpResponse::WatchStarted { watch_id }) => Ok(watch_id),
        other => Err(format!("expected a started watch, got {other:?}").into()),
    }
}

async fn checkpointed(tracking: &Tracking, source: &Fake, path: &str) -> Result<String, Box<dyn Error>> {
    match tracking.serve("lease", &checkpoint(path), source).await {
        Some(FileOpResponse::Checkpointed { checkpoint_id, .. }) => Ok(checkpoint_id),
        other => Err(format!("expected a checkpoint, got {other:?}").into()),
    }
}

fn events_of(
    response: Option<FileOpResponse>,
) -> Result<(Vec<FileWatchEvent>, u64, FileWatchOverflow), Box<dyn Error>> {
    match response {
        Some(FileOpResponse::WatchEvents {
            events,
            next_seq,
            overflowed,
        }) => Ok((events, next_seq, overflowed)),
        other => Err(format!("expected events, got {other:?}").into()),
    }
}

fn is_failure(response: &Option<FileOpResponse>, wanted: FileOpErrorKind) -> bool {
    matches!(response, Some(FileOpResponse::Failed { kind, .. }) if *kind == wanted)
}

#[tokio::test]
async fn other_verbs_are_not_served() {
    let tracking = Tracking::new();
    let source = Fake::default();
    let op = FileOp::Stat { path: "x".into() };
    assert_eq!(tracking.serve("lease", &op, &source).await, None);
}

#[tokio::test]
async fn a_checkpoint_diffs_against_the_tree_as_it_is_now() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/keep", 1), ("/w/gone", 1), ("/w/edit", 1)]);
    let id = checkpointed(&tracking, &source, "/w").await?;
    source.set(tree(&[("/w/keep", 1), ("/w/new", 1), ("/w/edit", 2)]));
    let reply = tracking.serve("lease", &changes(Some(&id), None), &source).await;
    assert_eq!(
        reply,
        Some(FileOpResponse::Changes {
            since: id.clone(),
            added: vec!["/w/new".into()],
            modified: vec!["/w/edit".into()],
            deleted: vec!["/w/gone".into()],
            truncated: FileResultTruncation::Complete,
        })
    );
    let narrowed = tracking
        .serve("lease", &changes(Some(&id), Some("/w/new")), &source)
        .await;
    assert!(matches!(
        narrowed,
        Some(FileOpResponse::Changes { ref added, ref modified, .. }) if added.len() == 1 && modified.is_empty()
    ));
    Ok(())
}

#[tokio::test]
async fn checkpoints_are_per_lease_and_bounded() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/a", 1)]);
    let first = checkpointed(&tracking, &source, "/w").await?;
    for _ in 0..Tracking::MAX_CHECKPOINTS_PER_LEASE {
        checkpointed(&tracking, &source, "/w").await?;
    }
    let evicted = tracking.serve("lease", &changes(Some(&first), None), &source).await;
    assert!(is_failure(&evicted, FileOpErrorKind::UnknownCheckpoint), "{evicted:?}");
    let other_lease = tracking.serve("other", &changes(Some(&first), None), &source).await;
    assert!(is_failure(&other_lease, FileOpErrorKind::UnknownCheckpoint));
    Ok(())
}

#[tokio::test]
async fn source_failures_become_replies() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/a", 1)]);
    let id = checkpointed(&tracking, &source, "/w").await?;
    let watch_id = started(&tracking, &source, "/w").await?;
    source.fail.store(true, Ordering::SeqCst);
    assert!(matches!(
        tracking.serve("lease", &checkpoint("/w"), &source).await,
        Some(FileOpResponse::NotFound { .. })
    ));
    assert!(matches!(
        tracking.serve("lease", &changes(Some(&id), None), &source).await,
        Some(FileOpResponse::NotFound { .. })
    ));
    assert!(matches!(
        tracking.serve("lease", &watch_start("/w"), &source).await,
        Some(FileOpResponse::NotFound { .. })
    ));
    assert!(matches!(
        tracking.serve("lease", &poll(&watch_id, 0, None), &source).await,
        Some(FileOpResponse::NotFound { .. })
    ));
    tracking.set_start_baseline("lease", "/w", tree(&[]));
    assert!(matches!(
        tracking.serve("lease", &changes(None, None), &source).await,
        Some(FileOpResponse::NotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn the_start_baseline_answers_changes_without_a_checkpoint() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/a", 1), ("/w/b", 1)]);
    tracking.set_start_baseline("lease", "/w", tree(&[("/w/a", 1)]));
    let reply = tracking.serve("lease", &changes(None, Some("/w")), &source).await;
    assert_eq!(
        reply,
        Some(FileOpResponse::Changes {
            since: FileOpResponse::CHANGES_SINCE_START.into(),
            added: vec!["/w/b".into()],
            modified: vec![],
            deleted: vec![],
            truncated: FileResultTruncation::Complete,
        })
    );
    Ok(())
}

#[tokio::test]
async fn the_provider_fills_in_paths_outside_the_baseline() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/a", 1)]);
    tracking.set_start_baseline("lease", "/w", tree(&[("/w/a", 1)]));
    source.provider(Ok(ManifestDiff {
        added: vec!["/etc/new".into(), "/w/ignored".into()],
        modified: vec![],
        deleted: vec![],
    }));
    let reply = tracking.serve("lease", &changes(None, None), &source).await;
    assert!(matches!(
        reply,
        Some(FileOpResponse::Changes { ref added, .. }) if added == &["/etc/new".to_string()]
    ));
    let inside = tracking.serve("lease", &changes(None, Some("/w/a")), &source).await;
    assert!(matches!(inside, Some(FileOpResponse::Changes { ref added, .. }) if added.is_empty()));
    source.provider(Err(FileOpFailure::invalid_request("provider broke")));
    let swallowed = tracking.serve("lease", &changes(None, None), &source).await;
    assert!(matches!(swallowed, Some(FileOpResponse::Changes { .. })));
    Ok(())
}

#[tokio::test]
async fn without_a_baseline_the_provider_or_an_error_answers() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::default();
    let none = tracking.serve("lease", &changes(None, None), &source).await;
    assert!(is_failure(&none, FileOpErrorKind::InvalidRequest), "{none:?}");
    source.provider(Ok(ManifestDiff {
        added: vec!["/x".into()],
        ..ManifestDiff::default()
    }));
    let served = tracking.serve("lease", &changes(None, None), &source).await;
    assert!(matches!(served, Some(FileOpResponse::Changes { ref added, .. }) if added.len() == 1));
    source.provider(Err(FileOpFailure::invalid_request("no")));
    let failed = tracking.serve("lease", &changes(None, None), &source).await;
    assert!(is_failure(&failed, FileOpErrorKind::InvalidRequest));
    let bare = tracking
        .serve("lease", &changes(None, None), &Bare(Manifest::new()))
        .await;
    assert!(is_failure(&bare, FileOpErrorKind::InvalidRequest));
    Ok(())
}

#[tokio::test]
async fn very_large_change_sets_are_cut_and_flagged() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::default();
    let id = checkpointed(&tracking, &source, "/w").await?;
    let many: Manifest = (0..=FileOp::MAX_ENTRIES)
        .map(|index| (format!("/w/{index:06}"), file(1)))
        .collect();
    source.set(many);
    let reply = tracking.serve("lease", &changes(Some(&id), None), &source).await;
    assert!(matches!(
        reply,
        Some(FileOpResponse::Changes { ref added, truncated, .. })
            if added.len() == FileOp::MAX_ENTRIES as usize && truncated == FileResultTruncation::Truncated
    ));
    Ok(())
}

#[tokio::test]
async fn a_watch_turns_scan_differences_into_ordered_events() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/keep", 1), ("/w/gone", 1), ("/w/edit", 1)]);
    let id = started(&tracking, &source, "/w").await?;
    assert_eq!(tracking.watch_ids("lease"), std::slice::from_ref(&id));

    let (idle, next, overflowed) = events_of(tracking.serve("lease", &poll(&id, 0, None), &source).await)?;
    assert!(idle.is_empty());
    assert_eq!((next, overflowed), (0, FileWatchOverflow::from(false)));

    let mut next_tree = tree(&[("/w/keep", 1), ("/w/new", 1), ("/w/edit", 2)]);
    next_tree.insert("/w/dir", ManifestEntry::directory());
    source.set(next_tree);
    let (events, next, _) = events_of(tracking.serve("lease", &poll(&id, 0, None), &source).await)?;
    let summary: Vec<_> = events
        .iter()
        .map(|e| (e.seq, e.kind, e.path.as_str(), e.is_dir))
        .collect();
    assert_eq!(
        summary,
        [
            (1, FileWatchEventKind::Deleted, "/w/gone", FileWatchTarget::File),
            (2, FileWatchEventKind::Created, "/w/dir", FileWatchTarget::Directory),
            (3, FileWatchEventKind::Created, "/w/new", FileWatchTarget::File),
            (4, FileWatchEventKind::Modified, "/w/edit", FileWatchTarget::File),
        ]
    );
    assert_eq!(next, 4);
    let (later, _, _) = events_of(tracking.serve("lease", &poll(&id, 2, None), &source).await)?;
    assert_eq!(later.len(), 2);

    assert_eq!(
        tracking
            .serve("lease", &FileOp::WatchStop { watch_id: id.clone() }, &source)
            .await,
        Some(FileOpResponse::WatchStopped {
            existed: FilePresence::Present
        })
    );
    assert_eq!(
        tracking
            .serve("lease", &FileOp::WatchStop { watch_id: id.clone() }, &source)
            .await,
        Some(FileOpResponse::WatchStopped {
            existed: FilePresence::Absent
        })
    );
    let gone = tracking.serve("lease", &poll(&id, 0, None), &source).await;
    assert!(is_failure(&gone, FileOpErrorKind::UnknownWatch));
    Ok(())
}

#[tokio::test]
async fn a_parked_poll_wakes_when_the_source_reports_a_change() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/a", 1)]);
    let id = started(&tracking, &source, "/w").await?;
    if let Ok(mut next) = source.after_wait.lock() {
        *next = Some(tree(&[("/w/a", 1), ("/w/b", 1)]));
    }
    let (events, _, _) = events_of(tracking.serve("lease", &poll(&id, 0, Some(5_000)), &source).await)?;
    assert_eq!(events.len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_default_wait_sleeps_and_gives_up_at_the_deadline() -> TestResult {
    let tracking = Tracking::new();
    let bare = Bare(tree(&[("/w/a", 1)]));
    let source = Fake::with(&[("/w/a", 1)]);
    let id = started(&tracking, &source, "/w").await?;
    let (events, next, _) = events_of(tracking.serve("lease", &poll(&id, 0, Some(30)), &bare).await)?;
    assert!(events.is_empty());
    assert_eq!(next, 0);
    assert_eq!(bare.scan_interval(), Duration::from_millis(250));
    assert!(bare.drain_hints("w").is_empty());
    assert_eq!(bare.changes_since_start().await, None);
    Ok(())
}

#[tokio::test]
async fn a_lease_holds_a_bounded_number_of_watches() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/a", 1)]);
    for _ in 0..Tracking::MAX_WATCHES_PER_LEASE {
        started(&tracking, &source, "/w").await?;
    }
    let reply = tracking.serve("lease", &watch_start("/w"), &source).await;
    assert!(is_failure(&reply, FileOpErrorKind::TooLarge), "{reply:?}");
    tracking.forget("lease");
    assert!(tracking.watch_ids("lease").is_empty());
    Ok(())
}

#[tokio::test]
async fn hints_recover_what_two_scans_cannot_tell_apart() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/rewritten", 1), ("/w/same", 1), ("/w/stat", 1)]);
    let id = started(&tracking, &source, "/w").await?;
    source.set(tree(&[("/w/rewritten", 1), ("/w/same", 1), ("/w/stat", 2)]));
    source.hint("/w/rewritten", FileWatchEventKind::Modified, FileWatchTarget::File);
    source.hint("/w/rewritten", FileWatchEventKind::Modified, FileWatchTarget::File);
    source.hint("/w/stat", FileWatchEventKind::Modified, FileWatchTarget::File);
    source.hint("/w/same", FileWatchEventKind::Created, FileWatchTarget::File);
    source.hint("/w/flash", FileWatchEventKind::Created, FileWatchTarget::File);
    source.hint("/w/flash", FileWatchEventKind::Created, FileWatchTarget::File);
    source.hint("/w/other", FileWatchEventKind::Deleted, FileWatchTarget::File);
    source.hint("/elsewhere/x", FileWatchEventKind::Created, FileWatchTarget::File);
    source.hint("/w", FileWatchEventKind::Modified, FileWatchTarget::Directory);
    let (events, _, _) = events_of(tracking.serve("lease", &poll(&id, 0, None), &source).await)?;
    let summary: Vec<_> = events.iter().map(|e| (e.kind, e.path.as_str())).collect();
    assert_eq!(
        summary,
        [
            (FileWatchEventKind::Modified, "/w/stat"),
            (FileWatchEventKind::Modified, "/w/rewritten"),
            (FileWatchEventKind::Created, "/w/flash"),
            (FileWatchEventKind::Deleted, "/w/flash"),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn an_overflowing_watch_reports_it_and_records_the_evidence() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::default();
    let id = started(&tracking, &source, "/w").await?;
    let burst =
        |from: usize, to: usize| -> Manifest { (from..to).map(|index| (format!("/w/{index:06}"), file(1))).collect() };
    source.set(burst(0, Tracking::WATCH_BUFFER_EVENTS + 100));
    let (events, _, overflowed) = events_of(tracking.serve("lease", &poll(&id, 0, None), &source).await)?;
    assert_eq!(events.len(), FileOp::WATCH_MAX_EVENTS_PER_POLL);
    assert!(overflowed.overflowed());
    let lease = LeaseId::new("lease");
    let evidence = tracking.take_overflow_evidence("lease", &lease);
    assert_eq!(evidence.len(), 1);
    assert!(evidence[0].0.contains("lease"));
    assert_eq!(
        evidence[0].1,
        FileWatchOverflowEvidence {
            watch_id: id.clone(),
            dropped_through: 100
        }
    );
    assert!(tracking.take_overflows("lease").is_empty());
    assert!(tracking.take_overflows("nobody").is_empty());
    Ok(())
}

#[tokio::test]
async fn a_repeated_overflow_replaces_the_pending_one() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::default();
    let id = started(&tracking, &source, "/w").await?;
    let burst = |count: usize| -> Manifest { (0..count).map(|index| (format!("/w/{index:06}"), file(1))).collect() };
    source.set(burst(Tracking::WATCH_BUFFER_EVENTS + 10));
    let newest = Tracking::WATCH_BUFFER_EVENTS as u64 + 10;
    tracking.serve("lease", &poll(&id, 0, None), &source).await;
    source.set(burst(2 * Tracking::WATCH_BUFFER_EVENTS + 50));
    tracking.serve("lease", &poll(&id, newest, None), &source).await;
    let pending = tracking.take_overflows("lease");
    assert_eq!(pending.len(), 1);
    assert!(pending[0].dropped_through > 10);
    Ok(())
}

#[tokio::test]
async fn two_parked_polls_apply_one_scan() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/a", 1)]);
    let id = started(&tracking, &source, "/w").await?;
    source.set(tree(&[("/w/a", 1), ("/w/b", 1)]));
    source.yield_once.store(true, Ordering::SeqCst);
    let first = poll(&id, 0, None);
    let second = poll(&id, 0, None);
    let (a, b) = tokio::join!(
        tracking.serve("lease", &first, &source),
        tracking.serve("lease", &second, &source)
    );
    let (a_events, _, _) = events_of(a)?;
    let (b_events, _, _) = events_of(b)?;
    assert_eq!(a_events.len(), 1);
    assert_eq!(a_events, b_events);
    Ok(())
}

#[tokio::test]
async fn a_watch_stopped_during_its_scan_is_not_resurrected() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/a", 1)]);
    let id = started(&tracking, &source, "/w").await?;
    source.yield_once.store(true, Ordering::SeqCst);
    let polling = poll(&id, 0, None);
    let stopping = FileOp::WatchStop { watch_id: id.clone() };
    let (polled, stopped) = tokio::join!(
        tracking.serve("lease", &polling, &source),
        tracking.serve("lease", &stopping, &source)
    );
    assert!(matches!(stopped, Some(FileOpResponse::WatchStopped { .. })));
    let (events, _, _) = events_of(polled)?;
    assert!(events.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_lease_forgotten_during_a_scan_is_not_resurrected() -> TestResult {
    let tracking = Tracking::new();
    let source = Fake::with(&[("/w/a", 1)]);
    let id = started(&tracking, &source, "/w").await?;
    source.yield_once.store(true, Ordering::SeqCst);
    let polling = poll(&id, 0, None);
    let forgetting = async {
        tracking.forget("lease");
    };
    let (polled, ()) = tokio::join!(tracking.serve("lease", &polling, &source), forgetting);
    let (events, _, _) = events_of(polled)?;
    assert!(events.is_empty());
    assert!(tracking.watch_ids("lease").is_empty());
    Ok(())
}

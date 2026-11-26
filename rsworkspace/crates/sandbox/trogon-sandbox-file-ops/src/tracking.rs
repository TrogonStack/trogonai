use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use trogon_sandbox_contract::{
    FileContentHashing, FileOp, FileOpErrorKind, FileOpResponse, FilePresence, FileRecursion, FileResultTruncation,
    FileWatchEvent, FileWatchEventKind, FileWatchOverflow, FileWatchOverflowEvidence, LeaseId,
};
use trogon_sandbox_protocol::subjects::file_watch_overflow_evidence;

use crate::manifest_diff::is_within;
use crate::{ChangeHint, FileOpFailure, Manifest, ManifestDiff, ManifestSource, WatchOverflow};

struct Checkpoint {
    path: String,
    hashing: FileContentHashing,
    manifest: Manifest,
}

struct StartBaseline {
    path: String,
    manifest: Manifest,
}

struct Watch {
    path: String,
    recursion: FileRecursion,
    last: Manifest,
    /// Bumped by each scan that is applied, so two parked polls cannot apply the same scan.
    generation: u64,
    next_seq: u64,
    events: VecDeque<FileWatchEvent>,
    /// Highest sequence number dropped from the buffer, 0 when none was.
    dropped_through: u64,
    touched: Instant,
}

#[derive(Default)]
struct LeaseTracking {
    start: Option<StartBaseline>,
    checkpoints: VecDeque<(String, Checkpoint)>,
    watches: HashMap<String, Watch>,
    /// Overflows not yet taken by [`Tracking::take_overflows`], one per watch.
    overflows: Vec<WatchOverflow>,
}

enum Buffered {
    Ready(FileOpResponse),
    Scan {
        path: String,
        recursion: FileRecursion,
        generation: u64,
    },
}

/// Worker-side checkpoints, lease-start baselines and watches for every lease the worker runs.
///
/// Change tracking and file watching are differences between two [`Manifest`]s:
///
/// - `checkpoint` stores a manifest worker-side and `changes` diffs it against the tree as it is
///   now.
/// - `watch` keeps the manifest of the last scan and turns each difference into create, modify
///   and delete events with increasing sequence numbers. A watch scans only while a poll is
///   parked, so an idle watch costs nothing and needs no background task. Events are therefore
///   the net change between scans: a file created and deleted between two scans produces no
///   event, and two writes between scans produce one modify. A source with a live change feed
///   passes what it saw as [`ChangeHint`]s, which recover the path created and deleted between
///   scans and the rewrite a fingerprint cannot see.
///
/// The worker supplies the manifests through [`ManifestSource`], so the same state machine serves
/// a host directory, a container reached by `exec`, or an in-memory map. State is keyed by lease,
/// which keeps one lease from naming another lease's checkpoint or watch.
#[derive(Default)]
pub struct Tracking {
    leases: Mutex<HashMap<String, LeaseTracking>>,
    ids: AtomicU64,
}

impl Tracking {
    /// Checkpoints kept per lease; creating one more evicts the oldest.
    pub const MAX_CHECKPOINTS_PER_LEASE: usize = 16;
    /// Watches live per lease at once.
    pub const MAX_WATCHES_PER_LEASE: usize = 16;
    /// Events a watch buffers before the oldest are dropped and the next poll reports overflow.
    pub const WATCH_BUFFER_EVENTS: usize = 4096;
    /// A watch nobody polled for this long is dropped, so an abandoned client cannot pin memory.
    pub const WATCH_IDLE_EXPIRY: Duration = Duration::from_secs(600);

    pub fn new() -> Self {
        Self::default()
    }

    fn leases(&self) -> std::sync::MutexGuard<'_, HashMap<String, LeaseTracking>> {
        self.leases.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn next_id(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.ids.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// Records the tree as it stands at lease start, the baseline `changes` uses when no
    /// checkpoint is named. Replaces an earlier one.
    pub fn set_start_baseline(&self, lease: &str, path: &str, manifest: Manifest) {
        self.leases().entry(lease.to_string()).or_default().start = Some(StartBaseline {
            path: path.to_string(),
            manifest,
        });
    }

    /// Takes the watch overflows recorded since the last call, one per watch, so the worker can
    /// put them on the lease's evidence timeline. The poll reply already says `overflowed`, but
    /// only to a caller still polling.
    pub fn take_overflows(&self, lease: &str) -> Vec<WatchOverflow> {
        self.leases()
            .get_mut(lease)
            .map(|state| std::mem::take(&mut state.overflows))
            .unwrap_or_default()
    }

    /// The overflows recorded since the last call under `key`, as the evidence subject of `lease`
    /// and the record to publish there. Workers that key tracking state by something other than
    /// the lease id pass both.
    pub fn take_overflow_evidence(&self, key: &str, lease: &LeaseId) -> Vec<(String, FileWatchOverflowEvidence)> {
        let subject = file_watch_overflow_evidence(lease);
        self.take_overflows(key)
            .into_iter()
            .map(|overflow| {
                (
                    subject.clone(),
                    FileWatchOverflowEvidence {
                        watch_id: overflow.watch_id,
                        dropped_through: overflow.dropped_through,
                    },
                )
            })
            .collect()
    }

    /// Ids of the watches live on `lease`, so a worker can stop the feeds of watches that were
    /// stopped or expired.
    pub fn watch_ids(&self, lease: &str) -> Vec<String> {
        self.leases()
            .get(lease)
            .map(|state| state.watches.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Drops everything held for a lease that has ended.
    pub fn forget(&self, lease: &str) {
        self.leases().remove(lease);
    }

    /// Serves a change-tracking or watch verb. `None` means `op` is neither, so the caller serves
    /// it some other way.
    pub async fn serve<S: ManifestSource>(&self, lease: &str, op: &FileOp, source: &S) -> Option<FileOpResponse> {
        self.expire_idle_watches(lease);
        Some(match op {
            FileOp::Checkpoint { path, hash } => self.checkpoint(lease, path, *hash, source).await,
            FileOp::Changes { since, path } => self.changes(lease, since.as_deref(), path.as_deref(), source).await,
            FileOp::WatchStart { path, recursive } => self.watch_start(lease, path, *recursive, source).await,
            FileOp::WatchPoll {
                watch_id,
                after_seq,
                wait_ms,
            } => self.watch_poll(lease, watch_id, *after_seq, *wait_ms, source).await,
            FileOp::WatchStop { watch_id } => {
                let existed = self
                    .leases()
                    .get_mut(lease)
                    .is_some_and(|state| state.watches.remove(watch_id).is_some());
                FileOpResponse::WatchStopped {
                    existed: FilePresence::from(existed),
                }
            }
            _ => return None,
        })
    }

    fn expire_idle_watches(&self, lease: &str) {
        if let Some(state) = self.leases().get_mut(lease) {
            state
                .watches
                .retain(|_, watch| watch.touched.elapsed() < Self::WATCH_IDLE_EXPIRY);
        }
    }

    async fn checkpoint<S: ManifestSource>(
        &self,
        lease: &str,
        path: &str,
        hashing: FileContentHashing,
        source: &S,
    ) -> FileOpResponse {
        let manifest = match source.manifest(path, FileRecursion::Recursive, hashing).await {
            Ok(manifest) => manifest,
            Err(failure) => return failure.into(),
        };
        let id = self.next_id("cp");
        let entries = manifest.len() as u64;
        let mut leases = self.leases();
        let state = leases.entry(lease.to_string()).or_default();
        state.checkpoints.push_back((
            id.clone(),
            Checkpoint {
                path: path.to_string(),
                hashing,
                manifest,
            },
        ));
        while state.checkpoints.len() > Self::MAX_CHECKPOINTS_PER_LEASE {
            state.checkpoints.pop_front();
        }
        FileOpResponse::Checkpointed {
            checkpoint_id: id,
            entries,
        }
    }

    async fn changes<S: ManifestSource>(
        &self,
        lease: &str,
        since: Option<&str>,
        path: Option<&str>,
        source: &S,
    ) -> FileOpResponse {
        match self.diff_since(lease, since, path, source).await {
            Ok((label, found)) => Self::changes_response(label, found, path),
            Err(failure) => failure.into(),
        }
    }

    async fn diff_since<S: ManifestSource>(
        &self,
        lease: &str,
        since: Option<&str>,
        path: Option<&str>,
        source: &S,
    ) -> Result<(String, ManifestDiff), FileOpFailure> {
        if let Some(id) = since {
            let baseline = {
                let leases = self.leases();
                leases.get(lease).and_then(|state| {
                    state
                        .checkpoints
                        .iter()
                        .find(|(checkpoint_id, _)| checkpoint_id == id)
                        .map(|(_, checkpoint)| {
                            (checkpoint.path.clone(), checkpoint.hashing, checkpoint.manifest.clone())
                        })
                })
            };
            let Some((checkpoint_path, hashing, before)) = baseline else {
                return Err(FileOpFailure::failed(
                    FileOpErrorKind::UnknownCheckpoint,
                    format!("checkpoint {id} is not held on this lease"),
                ));
            };
            let after = source
                .manifest(&checkpoint_path, FileRecursion::Recursive, hashing)
                .await?;
            return Ok((id.to_string(), before.diff(&after)));
        }
        let stored = {
            let leases = self.leases();
            leases
                .get(lease)
                .and_then(|state| state.start.as_ref())
                .map(|start| (start.path.clone(), start.manifest.clone()))
        };
        let found = match stored {
            Some((start_path, before)) => {
                let after = source
                    .manifest(&start_path, FileRecursion::Recursive, FileContentHashing::Metadata)
                    .await?;
                let mut found = before.diff(&after);
                // The baseline covers only its own tree. When the caller asks about more than
                // that, the provider's own diff fills in the rest.
                if !path.is_some_and(|requested| is_within(requested, &start_path))
                    && let Some(Ok(mut outside)) = source.changes_since_start().await
                {
                    outside.exclude(&start_path);
                    found.merge(outside);
                }
                found
            }
            None => match source.changes_since_start().await {
                Some(result) => result?,
                None => {
                    return Err(FileOpFailure::invalid_request(
                        "this lease holds no start baseline; create a checkpoint and name it in since",
                    ));
                }
            },
        };
        Ok((FileOpResponse::CHANGES_SINCE_START.to_string(), found))
    }

    fn changes_response(since: String, mut found: ManifestDiff, path: Option<&str>) -> FileOpResponse {
        if let Some(path) = path {
            found.restrict(path);
        }
        let ceiling = FileOp::MAX_ENTRIES as usize;
        let truncated = found.added.len() > ceiling || found.modified.len() > ceiling || found.deleted.len() > ceiling;
        found.added.truncate(ceiling);
        found.modified.truncate(ceiling);
        found.deleted.truncate(ceiling);
        FileOpResponse::Changes {
            since,
            added: found.added,
            modified: found.modified,
            deleted: found.deleted,
            truncated: FileResultTruncation::from(truncated),
        }
    }

    async fn watch_start<S: ManifestSource>(
        &self,
        lease: &str,
        path: &str,
        recursion: FileRecursion,
        source: &S,
    ) -> FileOpResponse {
        let last = match source.manifest(path, recursion, FileContentHashing::Metadata).await {
            Ok(manifest) => manifest,
            Err(failure) => return failure.into(),
        };
        let id = self.next_id("w");
        let mut leases = self.leases();
        let state = leases.entry(lease.to_string()).or_default();
        if state.watches.len() >= Self::MAX_WATCHES_PER_LEASE {
            return FileOpFailure::too_large(format!(
                "a lease may hold at most {} watches; stop one first",
                Self::MAX_WATCHES_PER_LEASE
            ))
            .into();
        }
        state.watches.insert(
            id.clone(),
            Watch {
                path: path.to_string(),
                recursion,
                last,
                generation: 0,
                next_seq: 1,
                events: VecDeque::new(),
                dropped_through: 0,
                touched: Instant::now(),
            },
        );
        FileOpResponse::WatchStarted { watch_id: id }
    }

    /// Events past `after_seq` already buffered, or what a scan needs when there are none. Also
    /// stamps the watch as used, and refuses an unknown watch.
    fn buffered(&self, lease: &str, watch_id: &str, after_seq: u64) -> Result<Buffered, FileOpFailure> {
        let mut leases = self.leases();
        let Some(watch) = leases.get_mut(lease).and_then(|state| state.watches.get_mut(watch_id)) else {
            return Err(FileOpFailure::failed(
                FileOpErrorKind::UnknownWatch,
                format!("watch {watch_id} is not live on this lease"),
            ));
        };
        watch.touched = Instant::now();
        let events: Vec<FileWatchEvent> = watch
            .events
            .iter()
            .filter(|event| event.seq > after_seq)
            .take(FileOp::WATCH_MAX_EVENTS_PER_POLL)
            .cloned()
            .collect();
        if events.is_empty() {
            return Ok(Buffered::Scan {
                path: watch.path.clone(),
                recursion: watch.recursion,
                generation: watch.generation,
            });
        }
        let next_seq = events.last().map_or(after_seq, |event| event.seq);
        Ok(Buffered::Ready(FileOpResponse::WatchEvents {
            events,
            next_seq,
            overflowed: FileWatchOverflow::from(watch.dropped_through > after_seq),
        }))
    }

    async fn watch_poll<S: ManifestSource>(
        &self,
        lease: &str,
        watch_id: &str,
        after_seq: u64,
        wait_ms: Option<u32>,
        source: &S,
    ) -> FileOpResponse {
        let wait = Duration::from_millis(u64::from(wait_ms.unwrap_or(0).min(FileOp::WATCH_MAX_WAIT_MS)));
        let deadline = Instant::now() + wait;
        loop {
            let (path, recursion, generation) = match self.buffered(lease, watch_id, after_seq) {
                Ok(Buffered::Ready(response)) => return response,
                Ok(Buffered::Scan {
                    path,
                    recursion,
                    generation,
                }) => (path, recursion, generation),
                Err(failure) => return failure.into(),
            };
            let hints = source.drain_hints(watch_id);
            let now = match source.manifest(&path, recursion, FileContentHashing::Metadata).await {
                Ok(manifest) => manifest,
                Err(failure) => return failure.into(),
            };
            self.apply_scan(lease, watch_id, generation, now, hints);
            if let Ok(Buffered::Ready(response)) = self.buffered(lease, watch_id, after_seq) {
                return response;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let overflowed = self.leases().get(lease).is_some_and(|state| {
                    state
                        .watches
                        .get(watch_id)
                        .is_some_and(|watch| watch.dropped_through > after_seq)
                });
                return FileOpResponse::WatchEvents {
                    events: Vec::new(),
                    next_seq: after_seq,
                    overflowed: FileWatchOverflow::from(overflowed),
                };
            }
            source.wait_for_watch(watch_id, &path, recursion, remaining).await;
        }
    }

    /// Turns the difference between the watch's last scan and `now` into buffered events. A scan
    /// that lost the race to another poll's scan is discarded.
    ///
    /// `hints` add two things the diff cannot see: a path the feed saw appear that is in neither
    /// scan (created and deleted between them) becomes a create and a delete, and a path in both
    /// scans with an identical fingerprint that the feed saw written or replaced becomes a
    /// modify.
    fn apply_scan(&self, lease: &str, watch_id: &str, generation: u64, now: Manifest, hints: Vec<ChangeHint>) {
        let mut leases = self.leases();
        let Some(lease_tracking) = leases.get_mut(lease) else {
            return;
        };
        let Some(watch) = lease_tracking.watches.get_mut(watch_id) else {
            return;
        };
        if watch.generation != generation {
            return;
        }
        watch.generation += 1;
        let dropped_before = watch.dropped_through;
        let changes = watch.last.diff(&now);
        let is_directory =
            |manifest: &Manifest, path: &str| manifest.get(path).map(|entry| entry.target()).unwrap_or_default();
        let mut emit = |kind: FileWatchEventKind, path: String, is_dir| {
            let seq = watch.next_seq;
            watch.next_seq += 1;
            watch.events.push_back(FileWatchEvent {
                seq,
                kind,
                path,
                is_dir,
            });
            if watch.events.len() > Self::WATCH_BUFFER_EVENTS
                && let Some(dropped) = watch.events.pop_front()
            {
                watch.dropped_through = dropped.seq;
            }
        };
        for path in changes.deleted {
            let target = is_directory(&watch.last, &path);
            emit(FileWatchEventKind::Deleted, path, target);
        }
        for path in changes.added {
            let target = is_directory(&now, &path);
            emit(FileWatchEventKind::Created, path, target);
        }
        let mut unseen_writes = Vec::new();
        let mut transient = Vec::new();
        for hint in hints {
            if !is_within(&hint.path, &watch.path) || hint.path == watch.path {
                continue;
            }
            match (watch.last.contains_path(&hint.path), now.contains_path(&hint.path)) {
                (false, false) if hint.kind == FileWatchEventKind::Created => {
                    transient.push((hint.path, hint.target));
                }
                (true, true) if hint.kind != FileWatchEventKind::Created && !changes.modified.contains(&hint.path) => {
                    unseen_writes.push(hint.path);
                }
                _ => {}
            }
        }
        unseen_writes.sort();
        unseen_writes.dedup();
        transient.sort_by(|a, b| a.0.cmp(&b.0));
        transient.dedup_by(|a, b| a.0 == b.0);
        for path in changes.modified.into_iter().chain(unseen_writes) {
            let target = is_directory(&now, &path);
            emit(FileWatchEventKind::Modified, path, target);
        }
        for (path, target) in transient {
            emit(FileWatchEventKind::Created, path.clone(), target);
            emit(FileWatchEventKind::Deleted, path, target);
        }
        watch.last = now;
        if watch.dropped_through > dropped_before {
            let overflow = WatchOverflow {
                watch_id: watch_id.to_string(),
                dropped_through: watch.dropped_through,
            };
            match lease_tracking
                .overflows
                .iter_mut()
                .find(|existing| existing.watch_id == watch_id)
            {
                Some(existing) => *existing = overflow,
                None => lease_tracking.overflows.push(overflow),
            }
        }
    }
}

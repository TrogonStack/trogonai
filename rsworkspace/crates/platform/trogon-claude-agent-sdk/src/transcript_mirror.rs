use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::{
    sync::{mpsc, oneshot},
    task::AbortHandle,
    time::{sleep, timeout},
};

use crate::{
    JsonObject, Message, SessionEntry, SessionKey, SessionKeyError, SessionStore, SessionStoreError,
    SessionStoreLimits, SessionStoreOptions, SystemMessage,
    constants::APPEND_BACKOFF_MS,
    session_store::{MirrorSizeEstimate, SessionStoreFlush},
};

#[derive(Debug, thiserror::Error)]
pub enum MirrorEnqueueError {
    #[error(transparent)]
    Key(#[from] SessionKeyError),
    #[error("transcript mirror worker stopped")]
    Closed,
}

#[derive(Debug, thiserror::Error)]
pub enum MirrorFailureError {
    #[error("session store transcript append failed")]
    Append {
        key: SessionKey,
        #[source]
        source: SessionStoreError,
    },
    #[error("session store transcript append timed out; it was not retried")]
    Timeout {
        key: SessionKey,
        /// Transcript path the frame was mirrored from.
        path: PathBuf,
        after_ms: u128,
    },
    #[error("additional session store failures were suppressed")]
    Suppressed { count: u64 },
}

impl MirrorFailureError {
    pub fn key(&self) -> Option<&SessionKey> {
        match self {
            Self::Append { key, .. } | Self::Timeout { key, .. } => Some(key),
            Self::Suppressed { .. } => None,
        }
    }

    /// Text upstream reports in the `error` field of the `mirror_error` system message.
    fn error_text(&self) -> String {
        match self {
            Self::Append { source, .. } => {
                let mut text = source.to_string();
                let mut current = std::error::Error::source(source);
                while let Some(next) = current {
                    text.push_str(": ");
                    text.push_str(&next.to_string());
                    current = next.source();
                }
                text
            }
            Self::Timeout { path, after_ms, .. } => {
                format!(
                    "SessionStore.append() timed out after {after_ms}ms for {}",
                    path.display()
                )
            }
            Self::Suppressed { count } => format!("{count} additional session store failures were suppressed"),
        }
    }

    pub fn into_message(self) -> Result<Message, serde_json::Error> {
        let mut data = JsonObject::new();
        data.insert("error".into(), self.error_text().into());
        data.insert("uuid".into(), uuid::Uuid::new_v4().to_string().into());
        let session_id = self.key().map(|key| key.session_id().clone());
        if let Some(key) = self.key() {
            let mut fields = JsonObject::new();
            fields.insert("project_key".into(), key.project_key().as_str().into());
            fields.insert("session_id".into(), key.session_id().as_str().into());
            if let Some(path) = key.subpath() {
                fields.insert("subpath".into(), path.as_str().into());
            }
            data.insert("key".into(), fields.into());
        }
        if let Self::Suppressed { count } = self {
            data.insert("suppressed_count".into(), count.into());
        }
        let mut raw = JsonObject::new();
        raw.insert("type".into(), "system".into());
        raw.insert("subtype".into(), "mirror_error".into());
        raw.extend(data.clone());
        Ok(Message::System(SystemMessage {
            subtype: "mirror_error".into(),
            session_id,
            data,
            event: crate::SystemEvent::MirrorError(crate::MirrorErrorMessage {
                key: raw.get("key").cloned(),
                error: self.error_text(),
            }),
            raw,
        }))
    }
}

enum MirrorCommand {
    Append {
        key: SessionKey,
        path: PathBuf,
        entries: Vec<SessionEntry>,
    },
    Flush(oneshot::Sender<Vec<MirrorFailureError>>),
}

pub struct MirrorFlush {
    receiver: oneshot::Receiver<Vec<MirrorFailureError>>,
    deadline: Option<tokio::time::Instant>,
    owner: Arc<WorkerOwner>,
}

impl MirrorFlush {
    /// Waits for the flush. Without [`SessionStoreLimits::flush_timeout`] this waits as long as the appends take, as
    /// upstream does; each append is still bounded by [`SessionStoreLimits::append_timeout`].
    pub async fn wait(self) -> Result<Vec<MirrorFailureError>, crate::SessionError> {
        let _owner = self.owner;
        let reported = match self.deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, self.receiver)
                .await
                .map_err(|_| crate::SessionError::MirrorFlushTimeout)?,
            None => self.receiver.await,
        };
        reported.map_err(|_| crate::SessionError::MirrorStopped)
    }
}

/// Buffers `transcript_mirror` frames and appends them to a [`SessionStore`].
///
/// Frames accumulate until a flush, or until the pending entries or bytes pass
/// [`SessionStoreLimits::max_pending_entries`] or [`SessionStoreLimits::max_pending_bytes`], which starts an eager
/// drain. Frames are never rejected for size. Each drain coalesces entries per key and appends them in order.
#[derive(Clone)]
pub struct TranscriptMirror {
    projects_dir: PathBuf,
    commands: mpsc::UnboundedSender<MirrorCommand>,
    flush_timeout: Option<std::time::Duration>,
    owner: Arc<WorkerOwner>,
}

struct WorkerOwner(AbortHandle);
impl Drop for WorkerOwner {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl std::fmt::Debug for TranscriptMirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptMirror").finish_non_exhaustive()
    }
}

impl TranscriptMirror {
    pub fn new(options: &SessionStoreOptions, projects_dir: PathBuf) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let worker = tokio::spawn(run_worker(
            options.store.clone(),
            receiver,
            options.limits.clone(),
            options.flush,
            options.size_estimate,
        ));
        Self {
            projects_dir,
            commands,
            flush_timeout: options.limits.flush_timeout.map(crate::Timeout::as_duration),
            owner: Arc::new(WorkerOwner(worker.abort_handle())),
        }
    }

    pub fn projects_dir(&self) -> &Path {
        &self.projects_dir
    }

    pub fn enqueue(&self, file_path: &Path, entries: Vec<SessionEntry>) -> Result<(), MirrorEnqueueError> {
        let key = SessionKey::from_transcript_path(file_path, &self.projects_dir)?;
        if entries.is_empty() {
            return Ok(());
        }
        self.commands
            .send(MirrorCommand::Append {
                key,
                path: file_path.to_owned(),
                entries,
            })
            .map_err(|_| MirrorEnqueueError::Closed)
    }

    pub async fn flush(&self) -> Result<Vec<MirrorFailureError>, crate::SessionError> {
        self.begin_flush()?.wait().await
    }

    pub fn begin_flush(&self) -> Result<MirrorFlush, crate::SessionError> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(MirrorCommand::Flush(reply))
            .map_err(|_| crate::SessionError::MirrorStopped)?;
        let now = tokio::time::Instant::now();
        Ok(MirrorFlush {
            receiver,
            // A timeout too long for an instant waits without a deadline.
            deadline: self.flush_timeout.and_then(|timeout| now.checked_add(timeout)),
            owner: self.owner.clone(),
        })
    }
}

#[derive(Default)]
struct Pending {
    batches: Vec<(SessionKey, PathBuf, Vec<SessionEntry>)>,
    entries: usize,
    bytes: usize,
}

impl Pending {
    fn push(&mut self, key: SessionKey, path: PathBuf, entries: Vec<SessionEntry>, estimate: MirrorSizeEstimate) {
        self.entries = self.entries.saturating_add(entries.len());
        self.bytes = self.bytes.saturating_add(encoded_len(&entries, estimate));
        match self.batches.iter_mut().find(|(_, candidate, _)| *candidate == path) {
            Some((_, _, batch)) => batch.extend(entries),
            None => self.batches.push((key, path, entries)),
        }
    }

    fn exceeds(&self, limits: &SessionStoreLimits) -> bool {
        self.entries > limits.max_pending_entries.get() || self.bytes > limits.max_pending_bytes.get()
    }
}

/// Estimated wire size of one frame: UTF-16 length of the compact JSON (TypeScript) or the character count of
/// Python's `json.dumps` with default separators and ASCII escaping.
fn encoded_len(entries: &[SessionEntry], estimate: MirrorSizeEstimate) -> usize {
    let objects: Vec<_> = entries.iter().map(SessionEntry::as_object).collect();
    match estimate {
        MirrorSizeEstimate::TypeScript => serde_json::to_string(&objects).map_or(0, |text| text.encode_utf16().count()),
        MirrorSizeEstimate::Python => {
            let mut out = Vec::new();
            let mut serializer = serde_json::Serializer::with_formatter(&mut out, crate::python_json::PythonFormatter);
            match serde::Serialize::serialize(&objects, &mut serializer) {
                Ok(()) => out.len(),
                Err(_) => 0,
            }
        }
    }
}

struct Failures {
    retained: Vec<MirrorFailureError>,
    suppressed: u64,
    /// `None` keeps every failure, as upstream reports each one.
    capacity: Option<usize>,
}

impl Failures {
    fn record(&mut self, failure: MirrorFailureError) {
        if self.capacity.is_none_or(|capacity| self.retained.len() < capacity) {
            self.retained.push(failure);
        } else {
            self.suppressed = self.suppressed.saturating_add(1);
        }
    }

    fn take(&mut self) -> Vec<MirrorFailureError> {
        if self.suppressed > 0 {
            self.retained.push(MirrorFailureError::Suppressed {
                count: std::mem::take(&mut self.suppressed),
            });
        }
        std::mem::take(&mut self.retained)
    }
}

async fn run_worker(
    store: Arc<dyn SessionStore>,
    mut receiver: mpsc::UnboundedReceiver<MirrorCommand>,
    limits: SessionStoreLimits,
    flush: SessionStoreFlush,
    estimate: MirrorSizeEstimate,
) {
    let mut pending = Pending::default();
    let mut failures = Failures {
        retained: Vec::new(),
        suppressed: 0,
        capacity: limits.max_retained_failures.map(crate::QueueCapacity::get),
    };
    while let Some(command) = receiver.recv().await {
        match command {
            MirrorCommand::Append { key, path, entries } => {
                pending.push(key, path, entries, estimate);
                if flush == SessionStoreFlush::Eager || pending.exceeds(&limits) {
                    drain(store.as_ref(), std::mem::take(&mut pending), &limits, &mut failures).await;
                }
            }
            MirrorCommand::Flush(reply) => {
                drain(store.as_ref(), std::mem::take(&mut pending), &limits, &mut failures).await;
                let _ = reply.send(failures.take());
            }
        }
    }
}

async fn drain(store: &dyn SessionStore, pending: Pending, limits: &SessionStoreLimits, failures: &mut Failures) {
    for (key, path, entries) in pending.batches {
        if let Some(failure) = append_with_retry(store, &key, &path, &entries, limits).await {
            tracing::error!(error = %failure, "transcript mirror append failed");
            failures.record(failure);
        }
    }
}

async fn append_with_retry(
    store: &dyn SessionStore,
    key: &SessionKey,
    path: &Path,
    entries: &[SessionEntry],
    limits: &SessionStoreLimits,
) -> Option<MirrorFailureError> {
    let mut delays = APPEND_BACKOFF_MS.into_iter();
    let mut attempt = 1_usize;
    loop {
        match timeout(limits.append_timeout.as_duration(), store.append(key, entries)).await {
            Ok(Ok(())) => return None,
            Ok(Err(source)) => {
                tracing::debug!(attempt, error = %source, "transcript mirror append attempt failed");
                let Some(delay) = delays.next() else {
                    return Some(MirrorFailureError::Append {
                        key: key.clone(),
                        source,
                    });
                };
                sleep(std::time::Duration::from_millis(delay)).await;
                attempt = attempt.saturating_add(1);
            }
            Err(_) => {
                return Some(MirrorFailureError::Timeout {
                    key: key.clone(),
                    path: path.to_owned(),
                    after_ms: limits.append_timeout.as_duration().as_millis(),
                });
            }
        }
    }
}

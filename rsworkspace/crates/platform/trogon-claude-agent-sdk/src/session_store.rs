use std::{
    collections::HashMap,
    future::Future,
    num::NonZeroUsize,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::adapters::{SystemClock, WallClock};
use crate::{
    QueueCapacity, SessionEntry, SessionId, SessionKey, SessionProjectKey, SessionSummaryEntry, SummaryDialect,
    Timeout, session_summary::fold_session_summary,
};

pub type SessionStoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, SessionStoreError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStoreOperation {
    Append,
    Load,
    List,
    ListSummaries,
    Delete,
    ListSubkeys,
}

impl std::fmt::Display for SessionStoreOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Append => "append",
            Self::Load => "load",
            Self::List => "list",
            Self::ListSummaries => "list summaries",
            Self::Delete => "delete",
            Self::ListSubkeys => "list subkeys",
        })
    }
}

#[derive(thiserror::Error)]
pub enum SessionStoreError {
    #[error("session store operation is unsupported: {0}")]
    Unsupported(SessionStoreOperation),
    #[error("session store backend failed")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("session store lock is poisoned")]
    Poisoned,
    #[error("session store clock exhausted")]
    ClockExhausted,
}

impl SessionStoreError {
    pub fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}

impl std::fmt::Debug for SessionStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionStoreError { .. }")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionListEntry {
    pub session_id: SessionId,
    pub modified_at_ms: u64,
}

/// External transcript storage. `append` must preserve call order and should deduplicate stable entry UUIDs.
pub trait SessionStore: Send + Sync {
    /// Reports whether the store implements an operation. `append` and `load` are required; a store that overrides
    /// an optional operation overrides this method to say so, which callers use instead of probing for
    /// [`SessionStoreError::Unsupported`].
    fn supports(&self, operation: SessionStoreOperation) -> bool {
        matches!(operation, SessionStoreOperation::Append | SessionStoreOperation::Load)
    }
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()>;
    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>>;
    fn list_sessions<'a>(&'a self, _project: &'a SessionProjectKey) -> SessionStoreFuture<'a, Vec<SessionListEntry>> {
        Box::pin(async { Err(SessionStoreError::Unsupported(SessionStoreOperation::List)) })
    }
    /// Returns incrementally maintained summaries for every session of a project in one call.
    ///
    /// Stores maintain these with [`fold_session_summary`](crate::fold_session_summary) inside `append`, skipping keys that
    /// carry a subpath, and stamp `modified_at_ms` with the same clock as [`SessionStore::list_sessions`].
    fn list_session_summaries<'a>(
        &'a self,
        _project: &'a SessionProjectKey,
    ) -> SessionStoreFuture<'a, Vec<SessionSummaryEntry>> {
        Box::pin(async { Err(SessionStoreError::Unsupported(SessionStoreOperation::ListSummaries)) })
    }
    fn delete<'a>(&'a self, _key: &'a SessionKey) -> SessionStoreFuture<'a, ()> {
        Box::pin(async { Err(SessionStoreError::Unsupported(SessionStoreOperation::Delete)) })
    }
    /// Lists subkeys below a main session key. Resume skips any entry that is not a safe [`SessionSubpath`](crate::SessionSubpath).
    fn list_subkeys<'a>(&'a self, _key: &'a SessionKey) -> SessionStoreFuture<'a, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[derive(Clone, Debug)]
pub struct SessionStoreLimits {
    /// Opt-in cap on the transcript mirror failures kept between flushes. Upstream reports every failure, so `None`
    /// keeps them all; with a cap, the rest are counted into one
    /// [`MirrorFailureError::Suppressed`](crate::MirrorFailureError::Suppressed).
    pub max_retained_failures: Option<QueueCapacity>,
    pub max_pending_entries: NonZeroUsize,
    pub max_pending_bytes: NonZeroUsize,
    /// Opt-in hardening for a store-backed resume. Upstream imposes none, so every default is unlimited.
    pub max_resume_entries: Option<NonZeroUsize>,
    pub max_resume_bytes: Option<NonZeroUsize>,
    pub max_subkeys: Option<QueueCapacity>,
    pub append_timeout: Timeout,
    /// Opt-in deadline for waiting on a transcript mirror flush. Upstream does not bound the flush beyond each
    /// append's own timeout, so `None` waits for it; a deadline fails the wait with
    /// [`SessionError::MirrorFlushTimeout`](crate::SessionError::MirrorFlushTimeout).
    pub flush_timeout: Option<Timeout>,
    pub load_timeout: Timeout,
}

impl Default for SessionStoreLimits {
    fn default() -> Self {
        Self {
            max_retained_failures: None,
            max_pending_entries: NonZeroUsize::new(500).unwrap_or(NonZeroUsize::MIN),
            max_pending_bytes: NonZeroUsize::new(1024 * 1024).unwrap_or(NonZeroUsize::MIN),
            max_resume_entries: None,
            max_resume_bytes: None,
            max_subkeys: None,
            append_timeout: Timeout::default(),
            flush_timeout: None,
            load_timeout: Timeout::default(),
        }
    }
}

/// When transcript mirror frames reach the store.
///
/// Upstream `sessionStoreFlush` / `session_store_flush`: `Batched` flushes on a result message or when the pending
/// buffer overflows; `Eager` appends every frame as soon as it is read, without coalescing across frames.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SessionStoreFlush {
    #[default]
    Batched,
    Eager,
}

/// How the mirror estimates the size of a pending frame against
/// [`SessionStoreLimits::max_pending_bytes`].
///
/// The TypeScript SDK counts UTF-16 code units of the compact JSON of the frame's entries; the Python SDK counts
/// characters of `json.dumps(entries)` with its default separators and ASCII escaping.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MirrorSizeEstimate {
    #[default]
    TypeScript,
    Python,
}

#[derive(Clone)]
pub struct SessionStoreOptions {
    pub store: Arc<dyn SessionStore>,
    pub config_directory: Option<PathBuf>,
    pub project_key: Option<SessionProjectKey>,
    pub limits: SessionStoreLimits,
    pub flush: SessionStoreFlush,
    pub size_estimate: MirrorSizeEstimate,
    /// The upstream SDK whose resume rules apply. Defaults to the TypeScript SDK.
    pub dialect: crate::SummaryDialect,
}

impl<S: SessionStore + 'static> From<Arc<S>> for SessionStoreOptions {
    fn from(store: Arc<S>) -> Self {
        Self::from(store as Arc<dyn SessionStore>)
    }
}

impl From<Arc<dyn SessionStore>> for SessionStoreOptions {
    fn from(store: Arc<dyn SessionStore>) -> Self {
        Self {
            store,
            config_directory: None,
            project_key: None,
            limits: SessionStoreLimits::default(),
            flush: SessionStoreFlush::default(),
            size_estimate: MirrorSizeEstimate::default(),
            dialect: crate::SummaryDialect::default(),
        }
    }
}

impl std::fmt::Debug for SessionStoreOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStoreOptions")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct MemoryState {
    entries: HashMap<SessionKey, Vec<SessionEntry>>,
    modified: HashMap<SessionKey, u64>,
    summaries: HashMap<(SessionProjectKey, SessionId), SessionSummaryEntry>,
    last_modified: u64,
}

/// Reference [`SessionStore`] that keeps everything in memory, for tests and development.
///
/// Like both upstream reference stores it does not deduplicate entry uuids, folds a [`SessionSummaryEntry`] on every
/// main-transcript append and stamps it with the write time that [`SessionStore::list_sessions`] reports.
#[derive(Clone, Default)]
pub struct InMemorySessionStore<C: WallClock<Time = SystemTime> = SystemClock> {
    state: Arc<Mutex<MemoryState>>,
    clock: C,
    dialect: SummaryDialect,
}

impl<C: WallClock<Time = SystemTime>> std::fmt::Debug for InMemorySessionStore<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemorySessionStore").finish_non_exhaustive()
    }
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_clock<C: WallClock<Time = SystemTime>>(clock: C) -> InMemorySessionStore<C> {
        InMemorySessionStore {
            state: Arc::default(),
            clock,
            dialect: SummaryDialect::default(),
        }
    }
}

impl<C: WallClock<Time = SystemTime>> InMemorySessionStore<C> {
    /// Chooses the summary `data` layout this store folds, TypeScript (camelCase) or Python (snake_case).
    pub fn with_summary_dialect(mut self, dialect: SummaryDialect) -> Self {
        self.dialect = dialect;
        self
    }

    /// Test helper: every entry stored under a key, empty when the key was never written.
    pub fn get_entries(&self, key: &SessionKey) -> Result<Vec<SessionEntry>, SessionStoreError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| SessionStoreError::Poisoned)?
            .entries
            .get(key)
            .cloned()
            .unwrap_or_default())
    }

    /// Test helper: number of stored main transcripts.
    pub fn size(&self) -> Result<usize, SessionStoreError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| SessionStoreError::Poisoned)?
            .entries
            .keys()
            .filter(|key| key.subpath().is_none())
            .count())
    }

    /// Test helper: removes every entry, summary and modification time.
    pub fn clear(&self) -> Result<(), SessionStoreError> {
        *self.state.lock().map_err(|_| SessionStoreError::Poisoned)? = MemoryState::default();
        Ok(())
    }

    fn write_time(&self, state: &mut MemoryState) -> Result<u64, SessionStoreError> {
        let clock = self
            .clock
            .now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .unwrap_or(0);
        let modified = clock.max(
            state
                .last_modified
                .checked_add(1)
                .ok_or(SessionStoreError::ClockExhausted)?,
        );
        state.last_modified = modified;
        Ok(modified)
    }
}

impl<C: WallClock<Time = SystemTime> + Send + Sync> SessionStore for InMemorySessionStore<C> {
    fn supports(&self, _operation: SessionStoreOperation) -> bool {
        true
    }

    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            if entries.is_empty() {
                return Ok(());
            }
            let mut state = self.state.lock().map_err(|_| SessionStoreError::Poisoned)?;
            state
                .entries
                .entry(key.clone())
                .or_default()
                .extend(entries.iter().cloned());
            let modified = self.write_time(&mut state)?;
            if key.subpath().is_none() {
                let summary_key = (key.project_key().clone(), key.session_id().clone());
                let mut folded =
                    fold_session_summary(self.dialect, state.summaries.get(&summary_key), key, entries, None);
                folded.modified_at_ms = modified;
                state.summaries.insert(summary_key, folded);
            }
            state.modified.insert(key.clone(), modified);
            Ok(())
        })
    }

    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async move {
            Ok(self
                .state
                .lock()
                .map_err(|_| SessionStoreError::Poisoned)?
                .entries
                .get(key)
                .cloned())
        })
    }

    fn list_sessions<'a>(&'a self, project: &'a SessionProjectKey) -> SessionStoreFuture<'a, Vec<SessionListEntry>> {
        Box::pin(async move {
            let state = self.state.lock().map_err(|_| SessionStoreError::Poisoned)?;
            let mut sessions: Vec<_> = state
                .entries
                .keys()
                .filter(|key| key.project_key() == project && key.subpath().is_none())
                .map(|key| SessionListEntry {
                    session_id: key.session_id().clone(),
                    modified_at_ms: state.modified.get(key).copied().unwrap_or(0),
                })
                .collect();
            sessions.sort_by_key(|session| std::cmp::Reverse(session.modified_at_ms));
            Ok(sessions)
        })
    }

    fn list_session_summaries<'a>(
        &'a self,
        project: &'a SessionProjectKey,
    ) -> SessionStoreFuture<'a, Vec<SessionSummaryEntry>> {
        Box::pin(async move {
            let state = self.state.lock().map_err(|_| SessionStoreError::Poisoned)?;
            let mut summaries: Vec<_> = state
                .summaries
                .iter()
                .filter(|((summary_project, _), _)| summary_project == project)
                .map(|(_, summary)| summary.clone())
                .collect();
            summaries.sort_by_key(|summary| std::cmp::Reverse(summary.modified_at_ms));
            Ok(summaries)
        })
    }

    fn delete<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().map_err(|_| SessionStoreError::Poisoned)?;
            if key.subpath().is_none() {
                state
                    .summaries
                    .remove(&(key.project_key().clone(), key.session_id().clone()));
            }
            let remove: Vec<_> = state
                .entries
                .keys()
                .filter(|candidate| {
                    if key.subpath().is_some() {
                        *candidate == key
                    } else {
                        candidate.project_key() == key.project_key() && candidate.session_id() == key.session_id()
                    }
                })
                .cloned()
                .collect();
            for key in remove {
                state.entries.remove(&key);
                state.modified.remove(&key);
            }
            Ok(())
        })
    }

    fn list_subkeys<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Vec<String>> {
        Box::pin(async move {
            let state = self.state.lock().map_err(|_| SessionStoreError::Poisoned)?;
            let mut paths: Vec<_> = state
                .entries
                .keys()
                .filter(|candidate| {
                    candidate.project_key() == key.project_key() && candidate.session_id() == key.session_id()
                })
                .filter_map(|key| key.subpath().map(|subpath| subpath.as_str().to_owned()))
                .collect();
            paths.sort();
            Ok(paths)
        })
    }
}

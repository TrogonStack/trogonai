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
use crate::{QueueCapacity, SessionEntry, SessionId, SessionKey, SessionProjectKey, Timeout};

pub type SessionStoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, SessionStoreError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStoreOperation {
    Append,
    Load,
    List,
    Delete,
    ListSubkeys,
}

impl std::fmt::Display for SessionStoreOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Append => "append",
            Self::Load => "load",
            Self::List => "list",
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
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()>;
    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>>;
    fn list_sessions<'a>(&'a self, _project: &'a SessionProjectKey) -> SessionStoreFuture<'a, Vec<SessionListEntry>> {
        Box::pin(async { Err(SessionStoreError::Unsupported(SessionStoreOperation::List)) })
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
    pub command_queue_capacity: QueueCapacity,
    pub max_pending_entries: NonZeroUsize,
    pub max_pending_bytes: NonZeroUsize,
    pub max_resume_entries: NonZeroUsize,
    pub max_resume_bytes: NonZeroUsize,
    pub max_subkeys: QueueCapacity,
    pub append_timeout: Timeout,
    pub flush_timeout: Timeout,
    pub load_timeout: Timeout,
}

impl Default for SessionStoreLimits {
    fn default() -> Self {
        Self {
            command_queue_capacity: QueueCapacity::default(),
            max_pending_entries: NonZeroUsize::new(500).unwrap_or(NonZeroUsize::MIN),
            max_pending_bytes: NonZeroUsize::new(1024 * 1024).unwrap_or(NonZeroUsize::MIN),
            max_resume_entries: NonZeroUsize::new(100_000).unwrap_or(NonZeroUsize::MIN),
            max_resume_bytes: NonZeroUsize::new(64 * 1024 * 1024).unwrap_or(NonZeroUsize::MIN),
            max_subkeys: QueueCapacity::default(),
            append_timeout: Timeout::default(),
            flush_timeout: Timeout::default(),
            load_timeout: Timeout::default(),
        }
    }
}

#[derive(Clone)]
pub struct SessionStoreOptions {
    pub store: Arc<dyn SessionStore>,
    pub config_directory: Option<PathBuf>,
    pub project_key: Option<SessionProjectKey>,
    pub limits: SessionStoreLimits,
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
    last_modified: u64,
}

#[derive(Clone, Default)]
pub struct InMemorySessionStore<C: WallClock<Time = SystemTime> = SystemClock> {
    state: Arc<Mutex<MemoryState>>,
    clock: C,
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
        }
    }
}

impl<C: WallClock<Time = SystemTime> + Send + Sync> SessionStore for InMemorySessionStore<C> {
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            if entries.is_empty() {
                return Ok(());
            }
            let mut state = self.state.lock().map_err(|_| SessionStoreError::Poisoned)?;
            let existing = state.entries.entry(key.clone()).or_default();
            for entry in entries {
                if entry
                    .uuid()
                    .is_some_and(|uuid| existing.iter().any(|prior| prior.uuid() == Some(uuid)))
                {
                    continue;
                }
                existing.push(entry.clone());
            }
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

    fn delete<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().map_err(|_| SessionStoreError::Poisoned)?;
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

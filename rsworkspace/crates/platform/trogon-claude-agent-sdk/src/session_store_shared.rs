//! The error, options and key helpers that the store-backed session helpers share with their TypeScript port.

use std::path::PathBuf;

use crate::{
    SessionId, SessionKey, SessionKeyError, SessionMutationError, SessionProjectKey, SessionProjectKeyError,
    SessionStoreError, SummaryDialect,
    constants::{DEFAULT_IMPORT_BATCH, LITE_READ_BUF_SIZE},
    session_files::LiteSessionFile,
};

#[derive(Debug, thiserror::Error)]
pub enum SessionStoreHelperError {
    #[error(transparent)]
    Mutation(#[from] SessionMutationError),
    #[error(transparent)]
    Store(#[from] SessionStoreError),
    #[error(transparent)]
    Project(#[from] SessionProjectKeyError),
    #[error(transparent)]
    Key(#[from] SessionKeyError),
    #[error("session store implements no {0}; it cannot serve this helper")]
    UnsupportedStore(&'static str),
    /// TypeScript `listSessions` on a store with neither `listSessionSummaries` nor `listSessions`.
    #[error(
        "sessionStore.listSessions is not implemented -- cannot list sessions. Provide a store with a listSessions() method."
    )]
    ListNotImplemented,
    /// TypeScript `listSubagents` on a store without `listSubkeys`.
    #[error(
        "sessionStore.listSubkeys is not implemented -- cannot list subagents. Provide a store with a listSubkeys() method."
    )]
    ListSubkeysNotImplemented,
    #[error(transparent)]
    Messages(#[from] crate::MessagesError),
    #[error("session file I/O failed")]
    Io(#[source] std::io::Error),
    #[error("session transcript line is not valid JSON")]
    Encoding(#[source] serde_json::Error),
    #[error("session transcript line is not a JSON object")]
    NotObject,
}

/// Arguments of [`list_sessions_from_store`](crate::list_sessions_from_store).
#[derive(Clone, Debug, Default)]
pub struct StoreListOptions {
    /// Project directory the key is derived from; the current directory when empty.
    pub directory: Option<PathBuf>,
    pub limit: Option<usize>,
    pub offset: usize,
    /// How the store's summary sidecars were folded; match the dialect the store was built with.
    pub summary_dialect: SummaryDialect,
}

/// Arguments of [`import_session_to_store`](crate::import_session_to_store).
#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub directory: Option<PathBuf>,
    pub include_subagents: bool,
    /// Entries per `append`; zero means the default of 500.
    pub batch_size: usize,
    /// Whose import rules apply: TypeScript keys the session by the project directory it names, Python by the name of
    /// the on-disk project directory the session was found in.
    pub dialect: SummaryDialect,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            directory: None,
            include_subagents: true,
            batch_size: DEFAULT_IMPORT_BATCH,
            dialect: SummaryDialect::TypeScript,
        }
    }
}

pub(crate) fn main_key(project: SessionProjectKey, session_id: &str) -> SessionKey {
    SessionKey::new(project, SessionId::from(session_id))
}

pub(crate) fn invalid_session(session_id: &str) -> SessionStoreHelperError {
    SessionMutationError::InvalidSessionId(session_id.to_owned()).into()
}

pub(crate) fn not_found(session_id: &str) -> SessionStoreHelperError {
    SessionMutationError::NotFound {
        session_id: session_id.to_owned(),
        directory: None,
    }
    .into()
}

/// Upstream `_jsonl_to_lite`: the same head and tail slices the disk path would read.
pub(crate) fn jsonl_to_lite(jsonl: &str, mtime: u64) -> LiteSessionFile {
    let bytes = jsonl.as_bytes();
    let size = bytes.len();
    let window = LITE_READ_BUF_SIZE as usize;
    let head = String::from_utf8_lossy(&bytes[..size.min(window)]).into_owned();
    let tail = if size > window {
        String::from_utf8_lossy(&bytes[size - window..]).into_owned()
    } else {
        head.clone()
    };
    LiteSessionFile {
        mtime,
        size: size as u64,
        head,
        tail,
    }
}

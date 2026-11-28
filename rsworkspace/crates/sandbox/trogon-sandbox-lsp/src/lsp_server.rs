use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::completion_item::CompletionItem;
use crate::completion_response::CompletionResponse;
use crate::document_path::DocumentPath;
use crate::document_symbols::DocumentSymbols;
use crate::initialize_result::InitializeResult;
use crate::inner::Inner;
use crate::listener::Listener;
use crate::listener_handle::ListenerHandle;
use crate::listener_kind::ListenerKind;
use crate::lock_ext::LockExt;
use crate::lsp_error::LspError;
use crate::lsp_launcher::LspLauncher;
use crate::lsp_params::{
    CompletionParams, ContentChange, DidChangeParams, DidOpenParams, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentParams, VersionedTextDocumentIdentifier,
};
use crate::lsp_server_options::LspServerOptions;
use crate::lsp_server_spec::LspServerSpec;
use crate::lsp_state::LspState;
use crate::notification_handler::NotificationHandler;
use crate::position::Position;
use crate::project_dir::ProjectDir;

/// A language server running as a detached command inside a sandbox.
pub struct LspServer {
    inner: Arc<Inner>,
}

impl LspServer {
    pub fn new(
        launcher: Arc<dyn LspLauncher>,
        spec: impl Into<LspServerSpec>,
        project_dir: ProjectDir,
        opts: LspServerOptions,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                launcher,
                spec: spec.into(),
                project_dir,
                opts,
                state: Mutex::new(LspState::Idle),
                runtime: Mutex::new(None),
                documents: Mutex::new(HashMap::new()),
                stderr_handlers: Mutex::new(Vec::new()),
                listeners: Mutex::new(Vec::new()),
                next_handler_id: AtomicU64::new(0),
                tasks: Mutex::new(Vec::new()),
                start_lock: tokio::sync::Mutex::new(()),
                started: OnceCell::new(),
                stopped: OnceCell::new(),
            }),
        }
    }

    pub fn project_dir(&self) -> &ProjectDir {
        &self.inner.project_dir
    }

    pub fn uri(&self, path: &DocumentPath) -> String {
        self.inner.uri(path)
    }

    /// Launches the server and performs the `initialize`/`initialized` handshake. Calling it
    /// again returns the first outcome.
    pub async fn start(&self) -> Result<InitializeResult, LspError> {
        let _guard = self.inner.start_lock.lock().await;
        if matches!(self.inner.state(), LspState::Stopping | LspState::Stopped) {
            return Err(LspError::NotRunning("language server was stopped".into()));
        }
        self.inner
            .started
            .get_or_init(|| Inner::do_start(Arc::clone(&self.inner)))
            .await
            .clone()
    }

    /// Shuts the server down: `shutdown`, `exit`, EOF, then TERM and KILL if it lingers.
    /// Safe to call more than once.
    pub async fn stop(&self) {
        self.inner.stopped.get_or_init(|| self.inner.do_stop()).await;
    }

    pub async fn did_open(&self, path: &DocumentPath, text: &str, language_id: Option<&str>) -> Result<(), LspError> {
        let uri = self.uri(path);
        let language_id = language_id
            .or(self.inner.spec.language_id.as_deref())
            .unwrap_or("plaintext");
        let connection = self.inner.running()?;
        self.inner.documents.locked().insert(uri.clone(), 1);
        let params = DidOpenParams {
            text_document: TextDocumentItem {
                uri: &uri,
                language_id,
                version: 1,
                text,
            },
        };
        connection
            .notify("textDocument/didOpen", Some(serde_json::to_value(&params)?))
            .await
    }

    pub async fn did_change(&self, path: &DocumentPath, text: &str) -> Result<(), LspError> {
        let uri = self.uri(path);
        let connection = self.inner.running()?;
        let version = {
            let mut documents = self.inner.documents.locked();
            let Some(current) = documents.get(&uri).copied() else {
                return Err(LspError::NotOpen { uri });
            };
            let version = current + 1;
            documents.insert(uri.clone(), version);
            version
        };
        let params = DidChangeParams {
            text_document: VersionedTextDocumentIdentifier { uri: &uri, version },
            content_changes: [ContentChange { text }],
        };
        connection
            .notify("textDocument/didChange", Some(serde_json::to_value(&params)?))
            .await
    }

    pub async fn did_close(&self, path: &DocumentPath) -> Result<(), LspError> {
        let uri = self.uri(path);
        let connection = self.inner.running()?;
        self.inner.documents.locked().remove(&uri);
        let params = TextDocumentParams {
            text_document: TextDocumentIdentifier { uri: &uri },
        };
        connection
            .notify("textDocument/didClose", Some(serde_json::to_value(&params)?))
            .await
    }

    pub async fn document_symbols(&self, path: &DocumentPath) -> Result<Option<DocumentSymbols>, LspError> {
        let uri = self.uri(path);
        let params = TextDocumentParams {
            text_document: TextDocumentIdentifier { uri: &uri },
        };
        self.request_as(
            "textDocument/documentSymbol",
            Some(serde_json::to_value(&params)?),
            None,
        )
        .await
    }

    pub async fn completions(&self, path: &DocumentPath, position: Position) -> Result<Vec<CompletionItem>, LspError> {
        let uri = self.uri(path);
        let params = CompletionParams {
            text_document: TextDocumentIdentifier { uri: &uri },
            position,
        };
        let response: Option<CompletionResponse> = self
            .request_as("textDocument/completion", Some(serde_json::to_value(&params)?), None)
            .await?;
        Ok(response.map(CompletionResponse::into_items).unwrap_or_default())
    }

    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, LspError> {
        self.inner.running()?.request(method, params, timeout).await
    }

    /// Like [`Self::request`], decoding the result into `T`. A `null` result decodes as `None`
    /// when `T` is an `Option`.
    pub async fn request_as<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<T, LspError> {
        let value = self.request(method, params, timeout).await?;
        serde_json::from_value(value).map_err(|e| LspError::Protocol(format!("unexpected {method} result: {e}")))
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), LspError> {
        self.inner.running()?.notify(method, params).await
    }

    /// Listeners may be added before [`Self::start`].
    pub fn on_notification(&self, method: &str, handler: impl Fn(&Value) + Send + Sync + 'static) -> ListenerHandle {
        let id = self.inner.next_handler_id.fetch_add(1, Ordering::Relaxed);
        let handler: NotificationHandler = Arc::new(handler);
        let connection = self
            .inner
            .runtime
            .locked()
            .as_ref()
            .map(|rt| Arc::clone(&rt.connection));
        let attached = connection.map(|c| c.on_notification(method, Arc::clone(&handler)));
        self.inner.listeners.locked().push(Listener {
            id,
            method: method.to_owned(),
            handler,
            attached,
        });
        ListenerHandle::new(Arc::clone(&self.inner), id, ListenerKind::Notification)
    }

    pub fn on_stderr(&self, handler: impl Fn(&str) + Send + Sync + 'static) -> ListenerHandle {
        let id = self.inner.next_handler_id.fetch_add(1, Ordering::Relaxed);
        self.inner.stderr_handlers.locked().push((id, Arc::new(handler)));
        ListenerHandle::new(Arc::clone(&self.inner), id, ListenerKind::Stderr)
    }
}

impl Drop for LspServer {
    /// Stops the background tasks. The remote process is left to [`LspServer::stop`] or the end
    /// of the sandbox lease.
    fn drop(&mut self) {
        self.inner.abort_tasks();
    }
}

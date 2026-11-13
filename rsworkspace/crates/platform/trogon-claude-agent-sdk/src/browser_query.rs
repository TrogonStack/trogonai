//! The browser entrypoint's `query()` and the accessors it exports for a query.

use std::{pin::Pin, sync::Arc};

use futures_util::Stream;
use indexmap::IndexMap;
use tokio_util::sync::CancellationToken;

use crate::{
    ClaudeAgentOptions, ClaudeSdkClient, DialogKind, ElicitationHandler, Error, HookRegistration, Message,
    OutputSchema, Query, SdkMcpServers, SseCcrEvent, SseHandle, SseOptions, SseTransport, ToolPermissionHandler,
    Transport, UserDialogHandler, UserInput, WebSocketOptions, WebSocketTransport,
};

/// TypeScript `prompt: AsyncIterable<SDKUserMessage>`.
pub type BrowserPrompt = Pin<Box<dyn Stream<Item = UserInput> + Send>>;

/// The `websocket | sse` arm of TypeScript `BrowserQueryOptions`. TypeScript's type allows exactly one; the enum
/// makes neither and both unrepresentable.
#[derive(Debug)]
pub enum BrowserTransport {
    WebSocket(WebSocketOptions),
    Sse(SseOptions),
}

/// TypeScript `BrowserQueryOptions`.
pub struct BrowserQueryOptions {
    pub prompt: BrowserPrompt,
    pub transport: BrowserTransport,
    /// `abortController`: cancelling it aborts the query.
    pub abort: Option<CancellationToken>,
    /// `canUseTool`.
    pub permission_handler: Option<Arc<dyn ToolPermissionHandler>>,
    pub hooks: Vec<HookRegistration>,
    /// `mcpServers`: the browser only hosts in-process servers.
    pub sdk_mcp_servers: SdkMcpServers,
    /// `jsonSchema`.
    pub json_schema: Option<OutputSchema>,
    pub elicitation_handler: Option<Arc<dyn ElicitationHandler>>,
    pub user_dialog_handler: Option<Arc<dyn UserDialogHandler>>,
    pub prompt_suggestions: Option<bool>,
    /// `supportedDialogKinds`: `None` sends nothing and `Some` of an empty list sends `[]`, as upstream. A
    /// non-empty list needs `user_dialog_handler`, or the query fails with [`Error::DialogKindsWithoutHandler`].
    pub supported_dialog_kinds: Option<Vec<DialogKind>>,
}

impl BrowserQueryOptions {
    pub fn new<S>(prompt: S, transport: BrowserTransport) -> Self
    where
        S: Stream<Item = UserInput> + Send + 'static,
    {
        Self {
            prompt: Box::pin(prompt),
            transport,
            abort: None,
            permission_handler: None,
            hooks: Vec::new(),
            sdk_mcp_servers: SdkMcpServers::new(),
            json_schema: None,
            elicitation_handler: None,
            user_dialog_handler: None,
            prompt_suggestions: None,
            supported_dialog_kinds: None,
        }
    }
}

impl std::fmt::Debug for BrowserQueryOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserQueryOptions")
            .field("transport", &self.transport)
            .field("hooks", &self.hooks.len())
            .field("sdk_mcp_servers", &self.sdk_mcp_servers.keys().collect::<Vec<_>>())
            .field("prompt_suggestions", &self.prompt_suggestions)
            .field("supported_dialog_kinds", &self.supported_dialog_kinds)
            .finish_non_exhaustive()
    }
}

/// TypeScript browser `query(options)`: a Claude Code query over a WebSocket or SSE connection.
///
/// The initialize request carries `hooks`, the SDK MCP server names, `jsonSchema`, `promptSuggestions` and
/// `supportedDialogKinds` and nothing else. The prompt is streamed in the background and ends the input when it
/// ends; a failed send aborts the query.
///
/// # Errors
///
/// [`Error::DialogKindsWithoutHandler`] before any connection when dialog kinds are declared without a handler,
/// or the connection and initialize errors of the transport.
pub async fn browser_query(options: BrowserQueryOptions) -> Result<Query, Error> {
    let BrowserQueryOptions {
        prompt,
        transport,
        abort,
        permission_handler,
        hooks,
        sdk_mcp_servers,
        json_schema,
        elicitation_handler,
        user_dialog_handler,
        prompt_suggestions,
        supported_dialog_kinds,
    } = options;
    let declare_dialog_kinds = supported_dialog_kinds.as_ref().is_some_and(Vec::is_empty);
    let agent = ClaudeAgentOptions {
        permission_handler,
        hooks,
        sdk_mcp_servers,
        output_schema: json_schema,
        elicitation_handler,
        user_dialog_handler,
        prompt_suggestions,
        supported_dialog_kinds: supported_dialog_kinds.unwrap_or_default(),
        capture_sdk_mcp_manifests: Some(false),
        ..ClaudeAgentOptions::default()
    };
    let token = abort.unwrap_or_default();
    match transport {
        BrowserTransport::WebSocket(websocket) => {
            let transport = WebSocketTransport::new(websocket).with_abort(token.clone());
            open(agent, transport, None, (prompt, token, declare_dialog_kinds)).await
        }
        BrowserTransport::Sse(sse) => {
            let transport = SseTransport::new(sse).with_abort(token.clone());
            let handle = transport.handle();
            open(agent, transport, Some(handle), (prompt, token, declare_dialog_kinds)).await
        }
    }
}

async fn open<T: Transport>(
    agent: ClaudeAgentOptions,
    transport: T,
    sse: Option<SseHandle>,
    (prompt, token, declare_dialog_kinds): (BrowserPrompt, CancellationToken, bool),
) -> Result<Query, Error> {
    let client = ClaudeSdkClient::connect_browser(agent, transport, declare_dialog_kinds).await?;
    let handle = client.handle();
    tokio::spawn(async move {
        if handle.stream_input(prompt).await.is_err() {
            token.cancel();
        }
    });
    Ok(Query::streaming(client, sse))
}

/// TypeScript `getSseDropCounts`: dropped frames per category for an SSE query, `None` for a WebSocket one.
#[must_use]
pub fn get_sse_drop_counts(query: &Query) -> Option<IndexMap<&'static str, u64>> {
    query.sse_handle().map(SseHandle::drop_counts)
}

/// TypeScript `getSseLastSequenceNum`: `None` for a WebSocket query and before any number is known.
#[must_use]
pub fn get_sse_last_sequence_num(query: &Query) -> Option<u64> {
    query.sse_handle().and_then(SseHandle::last_sequence_num)
}

/// TypeScript `getCcrEvent`: the envelope of the durable frame that delivered the message the transport read last,
/// `None` for a WebSocket query. TypeScript finds it by message object identity; a Rust message is moved to the
/// caller, so the envelope of the latest read stands in for it.
#[must_use]
pub fn get_ccr_event(query: &Query, _message: &Message) -> Option<SseCcrEvent> {
    query.sse_handle().and_then(SseHandle::last_ccr_event)
}

/// TypeScript `suspendSseStream`: false for a WebSocket, closed or suspended query.
#[must_use]
pub fn suspend_sse_stream(query: &Query) -> bool {
    query.sse_handle().is_some_and(SseHandle::suspend)
}

/// TypeScript `resumeSseStream`: false unless the query is suspended and still open.
#[must_use]
pub fn resume_sse_stream(query: &Query) -> bool {
    query.sse_handle().is_some_and(SseHandle::resume)
}

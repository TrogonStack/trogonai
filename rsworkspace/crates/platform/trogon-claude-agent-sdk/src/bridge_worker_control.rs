//! The control dispatch of the bridge worker (`t$` of the bridge bundle): what a client `control_request` is answered
//! with, for the replies the dispatcher builds on the spot.
//!
//! The attach path wires `onInterrupt`, `onSetModel`, `onSetMaxThinkingTokens`, `onSetPermissionMode`,
//! `onRenameSession`, `onStopTask`, `onBackgroundTasks`, `onSideQuestion` and the version string into the dispatcher,
//! and nothing else, so those are the only callbacks [`ControlCallbacks`] carries. A request that the upstream answers
//! after an asynchronous host call (`stop_task`, `background_tasks` and `side_question` with a registered callback)
//! comes back as [`BridgeControlReply::Deferred`] with what the responder needs, and a `set_model` or
//! `set_max_thinking_tokens` callback that returns a promise comes back as [`BridgeControlReply::Pending`] with that future;
//! `bridge_worker_control_async` finishes both.
//!
//! Branches the attach path never reaches (each is a draft row in the unreachable notes):
//! - the outbound-only rejection: an outbound-only session opens no read stream, so no request ever arrives;
//! - the process-exiting refusal (`G4`), a flag nothing in the bundle ever sets;
//! - `answerSharedControlRequest`, the `liveReply` marker (`e$`) and every callback the attach options do not name
//!   (`getInitializeState`, `getCommands`, `reportsInboundReadReceipts`, `onCancelAsyncMessage`,
//!   `onDialogKindsDeclared`, `onClientInitialize`, `onSetColor`, `onFileSuggestions`, `onReadFile`,
//!   `onGetWorkspaceDiff`, `onGetContextUsage`, `onMcp*`, `onForkConversation`, `hasPendingPrompts`): each is absent, so
//!   the request is answered as not supported and the registered branch cannot run.

use std::future::Future;
use std::pin::Pin;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::bridge_js::{clean_label, clean_reply_text};

mod constants;

use constants::{
    ERROR_CODE_UNSUPPORTED, FLAG_KEY_LIMIT, FLAG_KEYS_SHOWN, FLAG_SETTINGS, PERMISSION_MODES, THINKING_DISPLAYS,
};

/// What a host callback that reports success or failure returned. A callback that returns nothing is `None` where
/// the callback type says `Option<CallbackResult>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallbackResult {
    Ok,
    Failed {
        error: Option<String>,
        code: Option<String>,
    },
}

/// What a host promise rejected with: an abort (its message is shown to the client) or any other failure (only the
/// subtype is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    Aborted(String),
    Failed(String),
}

impl HostError {
    /// The text the host rejected with.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Aborted(message) | Self::Failed(message) => message,
        }
    }
}

pub type HostFuture<T> = Pin<Box<dyn Future<Output = Result<T, HostError>> + Send>>;

/// What a `set_model` promise resolved with. A promise that resolves with nothing is `None` where the callback type
/// says `Option<ModelVerdict>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelVerdict {
    pub ok: bool,
    pub model: Option<String>,
    /// The notices of the verdict that are text, in order, when `notices` was a list.
    pub notices: Option<Vec<String>>,
    /// The error text, when the error was text (anything else reads as an empty error).
    pub error: Option<String>,
    pub code: Option<String>,
}

pub type PendingModelCallback = Box<dyn FnMut(Option<&str>) -> HostFuture<Option<ModelVerdict>> + Send>;
pub type PendingThinkingTokensCallback =
    Box<dyn FnMut(Option<&Value>, Option<&Value>) -> HostFuture<Option<CallbackResult>> + Send>;

pub type ModelCallback = Box<dyn FnMut(Option<&str>) -> Option<CallbackResult> + Send>;
pub type ThinkingTokensCallback = Box<dyn FnMut(Option<&Value>, Option<&Value>) -> Option<CallbackResult> + Send>;
pub type TextCallback = Box<dyn FnMut(&str) -> Option<CallbackResult> + Send>;

/// The host callbacks the attach path hands to the dispatcher. The `has_*` flags stand for the callbacks whose reply
/// is asynchronous: they only say whether the host registered one.
#[derive(Default)]
pub struct ControlCallbacks {
    pub on_interrupt: Option<Box<dyn FnMut() + Send>>,
    pub on_set_model: Option<ModelCallback>,
    pub on_set_max_thinking_tokens: Option<ThinkingTokensCallback>,
    /// A `set_model` callback that returns a promise; it replaces `on_set_model` when both are set.
    pub on_set_model_pending: Option<PendingModelCallback>,
    /// A `set_max_thinking_tokens` callback that returns a promise; it replaces `on_set_max_thinking_tokens`.
    pub on_set_max_thinking_tokens_pending: Option<PendingThinkingTokensCallback>,
    pub on_set_permission_mode: Option<TextCallback>,
    pub on_rename_session: Option<TextCallback>,
    pub has_stop_task: bool,
    pub has_background_tasks: bool,
    pub has_side_question: bool,
}

/// The session facts a reply carries.
pub struct ControlSession<'a> {
    pub session_id: &'a str,
    pub claude_code_version: Option<&'a str>,
    pub pid: u32,
    /// The permission and dialog requests sent to the server that no response has settled yet (see
    /// [`is_pending_prompt`]).
    pub pending_prompts: &'a [Value],
}

/// A request whose reply waits for a host call.
#[derive(Clone, Debug, PartialEq)]
pub enum DeferredControl {
    StopTask { task_id: String },
    BackgroundTasks { tool_use_id: Option<String> },
    SideQuestion { request: Value },
}

/// A host promise the dispatcher already started; `bridge_worker_control_async::finish_pending` answers it.
pub enum PendingControl {
    SetModel(HostFuture<Option<ModelVerdict>>),
    SetMaxThinkingTokens(HostFuture<Option<CallbackResult>>),
}

impl std::fmt::Debug for PendingControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SetModel(_) => "PendingControl::SetModel",
            Self::SetMaxThinkingTokens(_) => "PendingControl::SetMaxThinkingTokens",
        })
    }
}

#[derive(Debug)]
pub enum BridgeControlReply {
    /// The `control_response` frame to write, with the session id already on it.
    Frame(Value),
    Deferred(DeferredControl),
    Pending(PendingControl),
}

/// Whether a request the host sent to the server is kept as pending until a response settles it: only the permission
/// and the dialog requests are.
#[must_use]
pub fn is_pending_prompt(request: &Value) -> bool {
    matches!(
        request.pointer("/request/subtype").and_then(Value::as_str),
        Some("can_use_tool" | "request_user_dialog")
    )
}

/// `Kt`: the cleaned text a control reply error carries.
#[must_use]
pub fn clean_control_text(text: &str) -> String {
    clean_reply_text(text)
}

/// `String(value)` for the values a request subtype can be (`None` is a missing key).
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_string(Some(item))
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
        Some(other) => other.to_string(),
    }
}

/// The `mcp_status` answer: no servers are attached.
#[derive(Serialize)]
struct McpStatus {
    #[serde(rename = "mcpServers")]
    mcp_servers: Vec<Value>,
}

/// The `fork_conversation` answer: forking needs the remote control server.
#[derive(Serialize)]
struct ForkConversation {
    forked: bool,
    error: &'static str,
    reason: &'static str,
}

/// The `mcp_set_servers` answer for a hint-only request: nothing changed.
#[derive(Serialize)]
struct McpSetServers {
    added: Vec<Value>,
    removed: Vec<Value>,
    errors: Map<String, Value>,
}

pub(crate) struct Replier<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) id: Option<&'a Value>,
}

impl Replier<'_> {
    fn frame(&self, subtype: &str, tail: Vec<(&str, Value)>) -> BridgeControlReply {
        BridgeControlReply::Frame(self.frame_value(subtype, tail))
    }

    /// The `control_response` frame with the request id after the subtype and `tail` after it.
    pub(crate) fn frame_value(&self, subtype: &str, tail: Vec<(&str, Value)>) -> Value {
        let mut response = Map::new();
        response.insert("subtype".to_owned(), Value::from(subtype));
        if let Some(id) = self.id {
            response.insert("request_id".to_owned(), id.clone());
        }
        for (key, value) in tail {
            response.insert(key.to_owned(), value);
        }
        let mut frame = Map::new();
        frame.insert("type".to_owned(), Value::from("control_response"));
        frame.insert("response".to_owned(), Value::Object(response));
        frame.insert("session_id".to_owned(), Value::from(self.session_id));
        Value::Object(frame)
    }

    fn success(&self) -> BridgeControlReply {
        self.frame("success", Vec::new())
    }

    /// A serialization failure answers as an error reply, the one path this reply already has.
    fn success_with(&self, response: &impl Serialize) -> BridgeControlReply {
        match serde_json::to_value(response) {
            Ok(response) => self.frame("success", vec![("response", response)]),
            Err(error) => self.error(&error.to_string()),
        }
    }

    fn error(&self, text: &str) -> BridgeControlReply {
        self.frame("error", vec![("error", Value::from(text))])
    }

    fn cleaned_error(&self, text: Option<&str>, code: Option<&str>) -> BridgeControlReply {
        let mut tail = vec![("error", Value::String(clean_control_text(text.unwrap_or_default())))];
        if let Some(code) = code {
            tail.push(("error_code", Value::from(code)));
        }
        self.frame("error", tail)
    }

    fn unsupported(&self, text: &str) -> BridgeControlReply {
        self.frame(
            "error",
            vec![
                ("error", Value::from(text)),
                ("error_code", Value::from(ERROR_CODE_UNSUPPORTED)),
            ],
        )
    }

    fn verdict(&self, result: Option<&CallbackResult>) -> BridgeControlReply {
        match result {
            Some(CallbackResult::Failed { error, .. }) => self.cleaned_error(error.as_deref(), None),
            _ => self.success(),
        }
    }
}

/// The reply to one client `control_request` (`request` is the whole frame, its `request_id` already normalized).
pub fn answer_control_request(
    request: &Value,
    session: &ControlSession<'_>,
    callbacks: &mut ControlCallbacks,
) -> BridgeControlReply {
    let replier = Replier {
        session_id: session.session_id,
        id: request.get("request_id"),
    };
    let body = request.get("request");
    let subtype_value = body.and_then(|body| body.get("subtype"));
    let field = |name: &str| body.and_then(|body| body.get(name));
    let missing = |what: &str| {
        replier.unsupported(&format!(
            "{what} is not supported in this context (callback not registered)"
        ))
    };
    match subtype_value.and_then(Value::as_str) {
        Some("initialize") => initialize(&replier, session),
        Some("set_model") => set_model(&replier, body, callbacks),
        Some("set_max_thinking_tokens") => set_max_thinking_tokens(&replier, body, callbacks),
        Some("set_permission_mode") => set_permission_mode(&replier, field("mode"), callbacks),
        Some("rename_session") => {
            let Some(title) = field("title").and_then(Value::as_str) else {
                return replier.error("rename_session: title must be a string");
            };
            match callbacks
                .on_rename_session
                .as_mut()
                .and_then(|callback| callback(title))
            {
                None => replier.unsupported(
                    "rename_session is not supported in this context (onRenameSession callback not registered)",
                ),
                Some(result) => replier.verdict(Some(&result)),
            }
        }
        Some("set_color") => {
            replier.unsupported("set_color is not supported in this context (onSetColor callback not registered)")
        }
        Some("file_suggestions") => replier.unsupported(
            "file_suggestions is not supported in this context (onFileSuggestions callback not registered)",
        ),
        Some("read_file") => {
            replier.unsupported("read_file is not supported in this context (onReadFile callback not registered)")
        }
        Some("get_workspace_diff") => replier.unsupported(
            "get_workspace_diff is not supported in this context (onGetWorkspaceDiff callback not registered)",
        ),
        Some("get_context_usage") => replier.unsupported(
            "get_context_usage is not supported in this context (onGetContextUsage callback not registered)",
        ),
        Some("get_usage") => replier.unsupported("get_usage is not supported in this context"),
        Some("mcp_status") => replier.success_with(&McpStatus {
            mcp_servers: Vec::new(),
        }),
        Some(name @ ("mcp_authenticate" | "mcp_oauth_callback_url" | "mcp_reconnect")) => missing(name),
        Some("interrupt") => {
            if let Some(callback) = callbacks.on_interrupt.as_mut() {
                callback();
            }
            replier.success()
        }
        Some("apply_flag_settings") => apply_flag_settings(&replier, field("settings")),
        Some("mcp_set_servers") => mcp_set_servers(&replier, body),
        Some("stop_task") => {
            let Some(task_id) = field("task_id").and_then(Value::as_str) else {
                return replier.error("stop_task: task_id must be a string");
            };
            if !callbacks.has_stop_task {
                return missing("stop_task");
            }
            BridgeControlReply::Deferred(DeferredControl::StopTask {
                task_id: task_id.to_owned(),
            })
        }
        Some("background_tasks") => {
            let tool_use_id = match field("tool_use_id") {
                None | Some(Value::Null) => None,
                Some(Value::String(text)) if text.is_empty() => None,
                Some(Value::String(text)) => Some(text.clone()),
                Some(_) => return replier.error("background_tasks: tool_use_id must be a string"),
            };
            if !callbacks.has_background_tasks {
                return missing("background_tasks");
            }
            BridgeControlReply::Deferred(DeferredControl::BackgroundTasks { tool_use_id })
        }
        Some("cancel_async_message") => {
            if field("message_uuid").and_then(Value::as_str).is_none() {
                return replier.error("cancel_async_message: message_uuid must be a string");
            }
            missing("cancel_async_message")
        }
        Some("side_question") => {
            if !body.is_some_and(is_side_question) {
                return replier.error(
                    "side_question: question must be a string and history an array of {question, response} strings",
                );
            }
            if !callbacks.has_side_question {
                return replier.unsupported(
                    "side_question is not supported in this context (onSideQuestion callback not registered)",
                );
            }
            BridgeControlReply::Deferred(DeferredControl::SideQuestion {
                request: body.cloned().unwrap_or(Value::Null),
            })
        }
        Some("fork_conversation") => replier.success_with(&ForkConversation {
            forked: false,
            error: "unsupported",
            reason: "requires_remote_control_server",
        }),
        _ => replier.unsupported(&format!(
            "REPL bridge does not handle control_request subtype: {}",
            clean_control_text(&js_string(subtype_value))
        )),
    }
}

fn initialize(replier: &Replier<'_>, session: &ControlSession<'_>) -> BridgeControlReply {
    let by_subtype = |wanted: &str| -> Vec<Value> {
        session
            .pending_prompts
            .iter()
            .filter(|prompt| prompt.pointer("/request/subtype").and_then(Value::as_str) == Some(wanted))
            .cloned()
            .collect()
    };
    let mut inner = Map::new();
    inner.insert("commands".to_owned(), Value::Array(Vec::new()));
    inner.insert("agents".to_owned(), Value::Array(Vec::new()));
    inner.insert("output_style".to_owned(), Value::from("normal"));
    inner.insert(
        "available_output_styles".to_owned(),
        Value::Array(vec![Value::from("normal")]),
    );
    inner.insert("models".to_owned(), Value::Array(Vec::new()));
    inner.insert("account".to_owned(), Value::Object(Map::new()));
    inner.insert("pid".to_owned(), Value::from(session.pid));
    if let Some(version) = session.claude_code_version {
        inner.insert("claude_code_version".to_owned(), Value::from(version));
    }
    replier.frame(
        "success",
        vec![
            ("response", Value::Object(inner)),
            ("pending_permission_requests", Value::Array(by_subtype("can_use_tool"))),
            (
                "pending_user_dialog_requests",
                Value::Array(by_subtype("request_user_dialog")),
            ),
        ],
    )
}

fn set_model(replier: &Replier<'_>, body: Option<&Value>, callbacks: &mut ControlCallbacks) -> BridgeControlReply {
    let model = body.and_then(|body| body.get("model"));
    if model.is_some_and(|model| !model.is_null() && !model.is_string()) {
        return replier.error("set_model: model must be a string");
    }
    if body
        .and_then(|body| body.get("only_if_model_is_current"))
        .is_some_and(|condition| *condition != Value::Bool(false))
    {
        return replier.error(
            "set_model: Remote Control does not implement only_if_model_is_current. The request changed nothing.",
        );
    }
    if let Some(callback) = callbacks.on_set_model_pending.as_mut() {
        return BridgeControlReply::Pending(PendingControl::SetModel(callback(model.and_then(Value::as_str))));
    }
    let result = callbacks
        .on_set_model
        .as_mut()
        .and_then(|callback| callback(model.and_then(Value::as_str)));
    replier.verdict(result.as_ref())
}

fn is_integer(value: &Value) -> bool {
    value
        .as_f64()
        .is_some_and(|number| number.is_finite() && number.fract() == 0.0)
}

fn set_max_thinking_tokens(
    replier: &Replier<'_>,
    body: Option<&Value>,
    callbacks: &mut ControlCallbacks,
) -> BridgeControlReply {
    let tokens = body.and_then(|body| body.get("max_thinking_tokens"));
    let display = body.and_then(|body| body.get("thinking_display"));
    let tokens_ok = tokens.is_none_or(|tokens| tokens.is_null() || (tokens.is_number() && is_integer(tokens)));
    let display_ok = display.is_none_or(|display| {
        display.is_null() || display.as_str().is_some_and(|text| THINKING_DISPLAYS.contains(&text))
    });
    if !body.is_some_and(Value::is_object) || !tokens_ok || !display_ok {
        return replier.error(
            "set_max_thinking_tokens: max_thinking_tokens must be an integer or null and thinking_display must be \"summarized\", \"omitted\", \"highlights\", or null",
        );
    }
    if let Some(callback) = callbacks.on_set_max_thinking_tokens_pending.as_mut() {
        return BridgeControlReply::Pending(PendingControl::SetMaxThinkingTokens(callback(tokens, display)));
    }
    let result = callbacks
        .on_set_max_thinking_tokens
        .as_mut()
        .and_then(|callback| callback(tokens, display));
    replier.verdict(result.as_ref())
}

fn set_permission_mode(
    replier: &Replier<'_>,
    mode: Option<&Value>,
    callbacks: &mut ControlCallbacks,
) -> BridgeControlReply {
    let named = mode
        .and_then(Value::as_str)
        .map(|text| if text == "manual" { "default" } else { text })
        .and_then(|text| PERMISSION_MODES.iter().copied().find(|known| *known == text));
    let result = match named {
        None => CallbackResult::Failed {
            error: Some(format!(
                "Cannot set permission mode: must be one of {}",
                PERMISSION_MODES.join(", ")
            )),
            code: Some("invalid_mode".to_owned()),
        },
        Some(name) => callbacks
            .on_set_permission_mode
            .as_mut()
            .and_then(|callback| callback(name))
            .unwrap_or_else(|| {
                CallbackResult::Failed {
                error: Some(
                    "set_permission_mode is not supported in this context (onSetPermissionMode callback not registered)"
                        .to_owned(),
                ),
                code: Some(ERROR_CODE_UNSUPPORTED.to_owned()),
            }
            }),
    };
    match result {
        CallbackResult::Ok => replier.success(),
        CallbackResult::Failed { error, code } => replier.cleaned_error(error.as_deref(), code.as_deref()),
    }
}

/// `qPe` followed by the missing handler: the settings are checked on the spot and the attach path registers no
/// `onApplyFlagSettings`, so a valid object is answered as not supported.
fn apply_flag_settings(replier: &Replier<'_>, settings: Option<&Value>) -> BridgeControlReply {
    match check_flag_settings(settings) {
        Err(text) => replier.error(&text),
        Ok(()) => replier.unsupported(
            "apply_flag_settings is not supported in this context (onApplyFlagSettings callback not registered)",
        ),
    }
}

/// `Object.keys` order: array-index names ascending, then the rest as they were written.
fn js_key_order(map: &Map<String, Value>) -> Vec<&String> {
    let index = |key: &str| {
        key.parse::<u32>()
            .ok()
            .filter(|index| *index < u32::MAX && index.to_string() == key)
    };
    let mut indexed: Vec<(u32, &String)> = map
        .keys()
        .filter_map(|key| index(key).map(|index| (index, key)))
        .collect();
    indexed.sort_by_key(|(index, _)| *index);
    let named = map.keys().filter(|key| index(key).is_none());
    indexed.into_iter().map(|(_, key)| key).chain(named).collect()
}

fn check_flag_settings(settings: Option<&Value>) -> Result<(), String> {
    let Some(settings) = settings.and_then(Value::as_object) else {
        return Err("apply_flag_settings: settings must be an object".to_owned());
    };
    let unknown: Vec<&String> = js_key_order(settings)
        .into_iter()
        .filter(|key| !FLAG_SETTINGS.contains(&key.as_str()))
        .collect();
    if !unknown.is_empty() {
        let shown: Vec<String> = unknown
            .iter()
            .take(FLAG_KEYS_SHOWN)
            .map(|key| clean_label(key, FLAG_KEY_LIMIT))
            .collect();
        let rest = unknown.len() - shown.len();
        let more = if rest > 0 {
            format!(" (+{rest} more)")
        } else {
            String::new()
        };
        return Err(format!(
            "apply_flag_settings: {}{more} cannot be changed over Remote Control (only effortLevel and ultracode can)",
            shown.join(", ")
        ));
    }
    if settings
        .get("effortLevel")
        .is_some_and(|level| !level.is_null() && !level.is_string())
    {
        return Err("apply_flag_settings: effortLevel must be a string or null".to_owned());
    }
    if settings.get("ultracode").is_some_and(|flag| !flag.is_boolean()) {
        return Err("apply_flag_settings: ultracode must be a boolean".to_owned());
    }
    if settings.is_empty() {
        return Err("apply_flag_settings: nothing to apply \u{2014} only effortLevel and ultracode can be changed over Remote Control".to_owned());
    }
    Ok(())
}

fn mcp_set_servers(replier: &Replier<'_>, body: Option<&Value>) -> BridgeControlReply {
    let servers = body.and_then(|body| body.get("servers"));
    let hint = body.and_then(|body| body.get("hint"));
    if servers.is_none() && hint.is_some_and(Value::is_string) {
        return replier.success_with(&McpSetServers {
            added: Vec::new(),
            removed: Vec::new(),
            errors: Map::new(),
        });
    }
    if !servers.is_some_and(Value::is_object) {
        return replier.cleaned_error(Some("mcp_set_servers: servers must be an object"), None);
    }
    replier.unsupported("mcp_set_servers is not supported in this context (onMcpSetServers callback not registered)")
}

/// `WPe`: a string `question` and an optional `history` array of `{question, response, fallback_notice?}` strings.
fn is_side_question(body: &Value) -> bool {
    let is_text = |value: Option<&Value>| value.is_some_and(Value::is_string);
    if !body.is_object() || !is_text(body.get("question")) {
        return false;
    }
    match body.get("history") {
        None => true,
        Some(Value::Array(history)) => history.iter().all(|turn| {
            turn.is_object()
                && is_text(turn.get("question"))
                && is_text(turn.get("response"))
                && turn.get("fallback_notice").is_none_or(Value::is_string)
        }),
        Some(_) => false,
    }
}

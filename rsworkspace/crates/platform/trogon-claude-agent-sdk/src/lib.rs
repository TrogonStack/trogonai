#![doc = include_str!("../README.md")]

pub mod adapters;
pub mod testing;
pub mod upstream;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

mod agent_definition;
mod agent_name;
mod argv;
mod auth_token;
mod backgrounded;
mod bridge_client;
#[cfg(test)]
mod bridge_fake_probe;
mod bridge_flavor;
mod bridge_git;
mod bridge_gitcmd;
mod bridge_gitdir;
mod bridge_js;
mod bridge_packed;
mod bridge_paths;
mod bridge_platform;
mod bridge_refs;
mod bridge_session;
mod bridge_session_callbacks;
mod bridge_session_core;
mod bridge_session_handle;
mod bridge_session_runtime;
mod bridge_session_stream_events;
mod bridge_types;
mod bridge_unicode;
mod bridge_unicode_tables;
mod bridge_url;
mod bridge_worker;
mod bridge_worker_control;
mod bridge_worker_control_async;
mod bridge_worker_delivery;
mod bridge_worker_events;
mod bridge_worker_heartbeat;
mod bridge_worker_inbound;
mod bridge_worker_queue;
mod bridge_worker_stream;
mod bridge_worker_stream_driver;
mod bridge_worker_stream_http;
mod bridge_worker_types;
mod browser_query;
mod builtin_tool_name;
mod bundled_cli;
mod channel;
mod cli_discovery;
mod cli_not_found_error;
mod cli_path;
mod client;
mod client_app;
mod client_app_derivation;
mod config_value;
mod connector_folder;
mod connector_vetting;
mod constants;
mod control;
mod control_request_id;
mod debug_file;
mod deep_json;
mod dialog;
mod dialog_kind;
mod driver;
mod effort_level;
mod environment;
mod environment_plan;
mod error;
mod extract_from_bunfs;
mod float_digits;
mod framing;
mod home_directory;
mod hooks;
#[cfg(unix)]
mod inherited_descriptors;
mod js_array;
mod js_array_mark;
mod js_json;
mod js_object_order;
mod js_regex;
mod js_runtime;
mod js_text;
mod json_scan;
mod jsonrpc_message;
mod large_stack;
mod managed_settings_delivery;
mod mcp_config;
mod mcp_read_resource_response;
mod mcp_routing;
mod mcp_server_dialect;
mod mcp_server_name;
mod mcp_server_options;
mod mcp_server_tool_policy;
mod mcp_tool_arguments_error;
mod mcp_tool_input_schema;
mod mcp_tool_name;
mod mcp_tool_name_warning;
mod mcp_url;
mod mcp_wire_ids;
mod message;
mod message_decode;
mod message_id;
mod message_origin;
mod model_name;
mod node_managed_pass;
mod node_merge;
mod node_merge_call;
mod node_merge_cost;
mod node_merge_room;
mod node_merge_warmth;
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "node's path module is ported whole and replayed against recorded probes; the connector check reaches only part of it"
    )
)]
mod node_path;
mod node_path_join;
mod node_stack;
mod node_text_file;
mod option_error;
mod options;
mod output_schema;
mod permission;
mod permission_mode;
mod permission_suggestions;
mod permission_update;
mod plugin;
mod prewarm;
mod process_options;
mod process_transport;
mod prompt;
mod pydantic_int_string;
mod pydantic_validation_error;
mod python_arith;
mod python_c_stack;
mod python_code_point;
#[cfg(any(test, target_os = "macos", windows))]
mod python_fnmatch;
mod python_ftplib;
mod python_hash;
mod python_http_client;
mod python_idna;
mod python_json;
mod python_json_error;
mod python_json_load;
mod python_json_value;
mod python_jsonschema;
mod python_pprint;
mod python_re;
mod python_re_cache;
mod python_recursion_model;
mod python_set;
mod python_sort;
mod python_string;
mod python_text;
mod python_text_file;
mod python_unicode_decode_error;
mod python_urllib;
mod python_urlopen;
mod python_urlopen_proxy;
#[cfg(any(test, windows))]
mod python_urlopen_registry;
#[cfg(any(test, target_os = "macos"))]
mod python_urlopen_sysconf;
mod python_urlopen_text;
mod python_urlopen_tls;
mod python_urlopen_tls_gate;
mod python_value_repr;
mod queue_capacity;
mod raw_json_id;
mod remote_control;
mod remote_schema_error;
mod remote_schema_retriever;
#[cfg(unix)]
mod run_as;
mod run_end;
mod run_end_ceiling;
mod runtime_options;
mod schema_pointer;
mod sdk_mcp_choices;
mod sdk_mcp_js;
mod sdk_mcp_regexp;
mod sdk_mcp_server;
mod sdk_mcp_zod;
mod sdk_mcp_zod_issue;
mod sdk_mcp_zod_run;
mod sdk_tool;
mod sdk_tools;
mod sensitive_string;
mod session_entry;
mod session_error;
mod session_files;
mod session_files_ts;
mod session_fork_ts;
mod session_id;
mod session_info;
mod session_key;
mod session_messages_ts;
mod session_mutations;
mod session_path_component;
mod session_precompact;
mod session_probe_ts;
mod session_project_key;
mod session_prompt;
mod session_reader;
mod session_resume;
mod session_store;
mod session_store_helpers;
mod session_store_shared;
mod session_store_ts;
mod session_subpath;
mod session_summary;
mod session_transcript;
mod setting_source;
mod settings_dialect;
mod settings_exact;
mod settings_input;
mod settings_json;
mod settings_load_error;
mod settings_managed_reader;
mod settings_merge_spec;
mod settings_numbers;
mod settings_option;
mod settings_parse;
mod settings_parse_deep;
mod settings_policy_content;
mod settings_policy_marketplace;
mod settings_policy_parse;
mod settings_read;
mod settings_read_hooks;
mod settings_read_spec;
mod settings_resolve;
mod settings_resolve_admin;
mod settings_resolve_connector;
mod settings_resolve_env;
mod settings_resolve_error;
mod settings_resolve_parent;
mod settings_resolve_platform;
mod settings_resolve_platform_kind;
mod settings_resolve_policy;
mod settings_resolve_reread;
mod settings_resolve_restrict;
mod settings_resolve_sources;
mod settings_resolve_state;
mod settings_resolve_tier;
mod settings_resolve_values;
mod settings_resolve_writable;
mod settings_resolve_writable_document;
mod settings_sandbox;
mod settings_sandbox_schema;
mod settings_schema;
mod settings_schema_builtins;
mod settings_schema_hosts;
mod settings_schema_paths;
mod settings_schema_permission_rule;
mod settings_schema_ports;
mod settings_schema_regexp;
mod settings_strings;
mod shadow_warning;
mod skills;
mod sse_ccr_event;
mod sse_delivery_update;
mod sse_drop_category;
mod sse_drop_log;
mod sse_error;
mod sse_frame;
mod sse_frame_filter;
mod sse_http_error;
mod sse_log_record;
mod sse_options;
mod sse_parser;
mod sse_prepared_write;
mod sse_request_id_set;
mod sse_retry;
mod sse_transport;
mod sse_utf8_decoder;
mod sse_worker_markers;
mod startup_timeout;
mod stderr_redaction;
mod stderr_tail;
mod summary_dialect;
mod system_prompt;
mod task_id;
mod timeout;
mod tool_config;
mod tool_name;
mod tool_rule;
mod tool_use_id;
mod tool_view;
mod transcript_mirror;
mod transport;
mod transport_cause;
mod typed_mcp_tool_handler;
mod ui_host;
mod urlopen_retriever;
mod urlopen_retriever_options;
mod usd_budget;
mod user_input;
mod user_input_priority;
mod validation_choices;
mod version_check;
mod websocket_error;
mod websocket_transport;
mod windows_path;
mod wire;
mod workspace_trust;

pub use agent_definition::{AgentDefinition, AgentEffort, AgentExecution, AgentMcpServerSpec};
pub use agent_name::AgentName;
pub use auth_token::{
    HostAuthToken, HostAuthTokenFuture, HostAuthTokenHandler, HostAuthTokenRequest, OAuthDeclineReason,
    OAuthTokenFuture, OAuthTokenHandler, OAuthTokenRequest, OAuthTokenResponse, OAuthTokenState,
};
pub use backgrounded::{Backgrounded, BackgroundedError};
pub use bridge_client::BridgeClient;
pub use bridge_git::{BranchDropLogDedup, CodeSessionGitContext, DefaultGitProbe, GitContextProbe, GitRepository};
pub use bridge_gitcmd::GitProbeContext;
pub use bridge_paths::PathGuard;
pub use bridge_platform::{EntryKind, HostProbe, Platform, PlatformProbe, ProbeError};
pub use bridge_session::{
    AttachBridgeSessionOptions, AttachCallbacks, AttachError, BridgeSession, CLOSE_INIT_FAILED, EnvironmentLookup,
    attach_bridge_session,
};
pub use bridge_session_callbacks::{CloseCallback, InboundMessageCallback, PermissionResponseCallback};
pub use bridge_session_handle::{ReconnectError, ReconnectOptions};
pub use bridge_types::{
    CreateCodeSessionOptions, CreateSessionFailure, CreateSessionOutcome, CredentialsFailure, CredentialsRejection,
    DeviceRefusal, FailureSource, FetchRemoteCredentialsOptions, RemoteCredentials, RemoteCredentialsOutcome,
    RemoteMcpConfig, SessionState,
};
pub use bridge_worker::{
    AUTH_FAILURES_BEFORE_EXIT, DEFAULT_HEARTBEAT_INTERVAL_MS, HEARTBEAT_INTERVAL_CEILING_MS,
    HEARTBEAT_INTERVAL_FLOOR_MS, HEARTBEAT_TIMEOUT_CEILING_MS, HEARTBEAT_TIMEOUT_FLOOR_MS, HEARTBEAT_TIMEOUT_MARGIN_MS,
    InvalidSessionIdError, NOT_FOUND_BEFORE_EXIT, NonOrigin403, RefusalSource, StatusClass, TokioPacing, WorkerClient,
    WorkerConfig, WorkerPacing, classify_status, external_metadata_patch, heartbeat_timeout, refusal_source,
    retry_after_ms, session_url, token_expiry,
};
pub use bridge_worker_control::{
    BridgeControlReply, CallbackResult, ControlCallbacks, ControlSession, DeferredControl, HostError, HostFuture,
    ModelCallback, ModelVerdict, PendingControl, PendingModelCallback, PendingThinkingTokensCallback, TextCallback,
    ThinkingTokensCallback, answer_control_request, clean_control_text, is_pending_prompt,
};
pub use bridge_worker_control_async::{
    AsyncControl, AsyncControlCallbacks, BackgroundTasksCallback, BackgroundTasksSwitch, ControlTask, FrameWriter,
    SIDE_QUESTION_DEADLINE, SideQuestionCallback, SideQuestionContext, StopTaskCallback, UuidSource,
    background_tasks_disabled,
};
pub use bridge_worker_delivery::{
    DeliveryAck, DeliveryNext, DeliveryQueue, DeliveryStatus, MAX_BATCH_ACKS, MAX_QUEUE_ACKS, delivery_body,
};
pub use bridge_worker_events::{
    Attempt, Batch, ClientEvent, EventNext, EventQueue, MAX_BATCH_BYTES, MAX_BATCH_EVENTS, MAX_QUEUE_EVENTS,
    POST_TIMEOUT_CEILING_MS, POST_TIMEOUT_FLOOR_MS, SHRINK_FROM_BYTES, lane_for_path, refused_for_good,
    retry_delay as event_retry_delay, upload_body, upload_timeout,
};
pub use bridge_worker_heartbeat::{HeartbeatDriver, PROBE_BEAT_SPACING, heartbeat_delay};
pub use bridge_worker_inbound::{INBOUND_DEDUP_CAPACITY, Inbound, InboundRouter, NOTIFICATION_ORIGIN, UuidSet};
pub use bridge_worker_queue::{
    Next, Patch, PatchQueue, Requeue, merge_patches, requeue_after_refusal, retry_delay, subtract_patches,
    without_status,
};
pub use bridge_worker_stream::{
    FrameParser, KEEPALIVE_SECONDS, LIVENESS_GRACE_MS, PERMANENT_STREAM_STATUSES, RECONNECT_BASE_MS, RECONNECT_CAP_MS,
    RECONNECT_JITTER_RATIO, Reopen, SLEEP_LATENESS_MS, StreamEffect, StreamOpen, StreamRequest, StreamSession,
    liveness_timeout, reconnect_delay,
};
pub use bridge_worker_stream_driver::{
    CONNECT_TIMEOUT, REFUSAL_BODY_TIMEOUT, REOPEN_AT_ONCE, StreamBody, StreamClocks, StreamConnector,
    StreamFailureError, StreamResponse, run_stream,
};
pub use bridge_worker_stream_http::{HttpStreamBody, HttpStreamConnector};
pub use bridge_worker_types::{
    InitError, InitFailureError, MismatchReason, RegisterFailureError, RequestFailure, RequestOutcome,
    RequiresActionDetails,
};
pub use browser_query::{
    BrowserPrompt, BrowserQueryOptions, BrowserTransport, browser_query, get_ccr_event, get_sse_drop_counts,
    get_sse_last_sequence_num, resume_sse_stream, suspend_sse_stream,
};
pub use builtin_tool_name::BuiltinToolName;
pub use bundled_cli::{
    NativeCliNotFoundError, NativePackageTarget, NodeModuleLookup, NodeModuleNotFoundError, ResolveNodeModule,
    bundled_cli_derives_client_app, find_bundled_cli, find_native_cli, meets_derived_client_app_minimum,
};
pub use cli_not_found_error::CliNotFoundError;
pub use cli_path::{CliPathError, WindowsBatchCli};
pub use client::{
    ClaudeSdkClient, ClientConnector, ClientHandle, ConnectPrompt, CustomTransport, NativeCli, Query,
    UnconnectedClient, query, query_turn, query_turn_with_adapters, query_turn_with_transport, query_with_adapters,
    query_with_transport,
};
pub use client_app::{format_client_app, normalize_client_app};
pub use client_app_derivation::{BundledCli, ClientAppDerivation};
pub use config_value::ConfigValue;
pub use constants::{DEFAULT_ENTRYPOINT, MINIMUM_CLAUDE_CODE_VERSION, SDK_VERSION};
pub use control::{
    ConnectionInfo, ContextUsageCategory, ContextUsageDetail, ContextUsageResponse, ControlReply, FeatureSupport,
    InterruptReceipt, MaxThinkingTokensDisplay, McpEnabled, McpPermissionModeOverride, McpSetServersResult,
    McpStatusResponse, ReadFileEncoding, ReadFileOptions, ReloadPluginsOptions, RewindFilesResult, RewindMode,
    SettingsUpdateSource,
};
pub use control_request_id::ControlRequestId;
pub use deep_json::{DeepJsonObject, DeepJsonValue};
pub use dialog::{
    ElicitationFuture, ElicitationHandler, ElicitationMode, ElicitationRequest, ElicitationResponse, UserDialogFuture,
    UserDialogHandler, UserDialogRequest, UserDialogResponse,
};
pub use dialog_kind::DialogKind;
pub use effort_level::EffortLevel;
pub use environment::Environment;
pub use environment_plan::EnvironmentPlan;
pub use error::{
    AbortError, ControlOperation, ControlRejectionError, Error, InitializeTimeoutError, JsonDecodeError,
    JsonDecodeSourceError, MessageParseError, ProtocolError, ResultError, python_cli_not_found_text,
    python_process_error_text,
};
pub use extract_from_bunfs::{extract_from_bunfs, extract_from_bunfs_with};
pub use home_directory::HomeDirectoryError;
pub use hooks::{
    CallbackError, HookContinuation, HookEvent, HookFailureMode, HookFuture, HookHandler, HookOutput,
    HookOutputVisibility, HookPermissionDecision, HookRegistration, HookRequest, HookSpecificOutput, SyncHookOutput,
};
pub use js_runtime::JsRuntime;
pub use managed_settings_delivery::ManagedSettingsDelivery;
pub use mcp_config::McpConfig;
pub use mcp_read_resource_response::McpReadResourceResponse;
pub use mcp_server_dialect::McpServerDialect;
pub use mcp_server_name::McpServerName;
pub use mcp_server_options::{McpRemoteServerOptions, McpSdkServerOptions, McpServerOptions, McpStdioServerOptions};
pub use mcp_server_tool_policy::{McpServerToolPolicy, McpToolOrgMaxPermission, McpToolPermissionPolicy};
pub use mcp_tool_arguments_error::McpToolArgumentsError;
pub use mcp_tool_input_schema::{McpToolInputSchema, McpToolInputSchemaError};
pub use mcp_tool_name::McpToolName;
pub use mcp_url::{McpUrl, McpUrlError};
pub use mcp_wire_ids::{McpWireIds, McpWireScan};
pub use message::{
    AssistantMessage, Content, ContentBlock, ConversationResetMessage, DeferredToolUse, HookEventMessage, Message,
    MessageDecoding, MirrorErrorMessage, PermissionDenial, RateLimitEvent, RateLimitInfo, ResultMessage, ResultStatus,
    StreamEvent, SystemEvent, SystemMessage, TaskNotificationMessage, TaskProgressMessage, TaskStartedMessage,
    TaskUpdatedMessage, ToolResultStatus, UserMessage,
};
pub use message_id::MessageId;
pub use message_origin::{MessageOrigin, MessageOriginError};
pub use model_name::ModelName;
pub use node_path::NodePathDialect;
pub use node_stack::NodeStack;
pub use option_error::OptionError;
pub use options::{
    ClaudeAgentOptions, ContinueConversation, Customizations, DuplicateControlRequests, EndWithoutResult,
    FileCheckpointing, InitializeTimeout, InputAfterFinish, Limits, McpDiscovery, McpServers, MessageBuffer,
    PartialMessages, RequestIdExhaustion, RunEndTimeout, SdkMcpServers, SessionFork, SessionPersistence, Shutdown,
    SystemPrompt, ToolSet, VerbatimPrompts,
};
pub use output_schema::{OutputSchema, OutputSchemaError};
pub use permission::{
    ComputerFolder, DenialDisposition, MatchedAskRule, McpServerProvenance, PermissionDecision,
    PermissionDecisionClassification, PermissionFuture, ToolPermissionHandler, ToolPermissionRequest,
};
pub use permission_mode::PermissionMode;
pub use permission_update::{
    PermissionBehavior, PermissionRuleValue, PermissionUpdate, PermissionUpdateDestination, PermissionUpdateError,
};
pub use plugin::{LocalPlugin, PluginDelivery};
pub use prewarm::{
    ClaimError, ClaimInfo, ClaimOptions, SpareProcess, StartupOptions, WarmQuery, prewarm, prewarm_with_adapters,
    startup, startup_with_adapters,
};
pub use process_options::{
    ArgvStyle, ClaudeCodeMarker, EnvironmentMode, ExtraArgument, OnProcessSpawned, ProcessSpawnInfo, ScriptRuntime,
    SpawnClaudeCodeProcess, SpawnOptions, StderrDelivery, StderrHandler, TraceContext,
};
pub use prompt::Prompt;
pub use python_c_stack::PythonCStack;
pub use python_code_point::{PythonCodePoint, PythonCodePointError};
pub use python_json_error::{
    ExactIntegerError, JsonContainer, JsonDecodeError as PythonJsonDecodeError, JsonDecodeErrorKind, PythonJsonError,
};
pub use python_json_load::python_json_loads;
pub use python_json_value::{ExactInteger, PythonJsonObject, PythonJsonValue, PythonNumber, python_json_dumps};
pub use python_string::{CodePoints, PythonString};
pub use python_unicode_decode_error::{PythonUnicodeDecodeError, UnicodeDecodeReason};
pub use queue_capacity::{QueueCapacity, QueueCapacityError};
pub use raw_json_id::{RawJsonId, RawJsonIdError};
pub use remote_control::{RemoteControlOptions, WorkSecretFuture, WorkSecretRefreshRequest, WorkSecretRefresher};
pub use remote_schema_error::RemoteSchemaError;
pub use remote_schema_retriever::RemoteSchemaRetriever;
pub use runtime_options::{
    DangerousPermissionSkip, DebugOutput, EmptyCollection, HookEvents, PermissionPrompts, SdkBeta, Skills, TaskBudget,
    ThinkingConfig, ThinkingDisplay,
};
pub use schema_pointer::{SchemaPointer, SchemaPointerError};
pub use sdk_mcp_choices::{
    LengthChoice, NullabilityChoice, NumberFormatChoice, ObjectChoice, RefineFailure, Refinement, SchemaCallback,
    SchemaCallbackOptions, SchemaChoice, StringCheckKind, StringFormatChoice, Transformation, UnionChoice,
};
pub use sdk_mcp_server::{
    LocalMcpServer, LocalMcpServerOptions, McpFuture, McpServerError, PythonMcpFuture, SdkMcpServer, SdkMcpSession,
};
pub use sdk_tool::{McpToolFuture, McpToolHandler, McpToolOutput, McpToolRequest, SdkTool, SdkToolOptions};
pub use sdk_tools::{SdkTools, SdkToolsError};
pub use sensitive_string::SensitiveString;
pub use session_entry::{SessionEntry, SessionEntryError};
pub use session_error::SessionError;
pub use session_files::{ListSessionsOptions, SessionFiles};
pub use session_id::SessionId;
pub use session_info::SdkSessionInfo;
pub use session_key::{SessionKey, SessionKeyError};
pub use session_messages_ts::{ChainAlgorithm, MessageOptions, MessagesError};
pub use session_mutations::{
    ForkAlgorithm, ForkSessionResult, JsonEscaping, SessionMutationError, delete_session, fork_session, rename_session,
    tag_session,
};
pub use session_project_key::{
    SessionProjectKey, SessionProjectKeyError, project_key_for_directory, project_key_for_directory_in,
};
pub use session_reader::{
    get_session_info, get_session_messages, get_subagent_messages, list_sessions, list_subagents,
};
pub use session_resume::{MaterializedSession, SessionPreparationOptions, SessionRuntime, StoreResume};
pub use session_store::{
    InMemorySessionStore, MirrorSizeEstimate, SessionListEntry, SessionStore, SessionStoreError, SessionStoreFlush,
    SessionStoreFuture, SessionStoreLimits, SessionStoreOperation, SessionStoreOptions,
};
pub use session_store_helpers::{
    delete_session_via_store, delete_session_via_store_with, fork_session_via_store, fork_session_via_store_with,
    get_session_info_from_store, get_session_info_from_store_with, get_session_messages_from_store,
    get_session_messages_from_store_with, get_subagent_messages_from_store, get_subagent_messages_from_store_with,
    import_session_to_store, list_sessions_from_store, list_subagents_from_store, list_subagents_from_store_with,
    rename_session_via_store, rename_session_via_store_with, tag_session_via_store, tag_session_via_store_with,
};
pub use session_store_shared::{ImportOptions, SessionStoreHelperError, StoreListOptions};
pub use session_subpath::{SessionSubpath, SessionSubpathError};
pub use session_summary::{SessionSummaryEntry, fold_session_summary, summary_entry_to_sdk_info};
pub use session_transcript::{MessageExtras, MessagePage, SessionMessage, SessionMessageKind};
pub use setting_source::SettingSource;
pub use settings_dialect::SettingsDialect;
pub use settings_exact::js_stringify_exact;
pub use settings_input::SettingsInput;
pub use settings_json::js_stringify;
pub use settings_load_error::{SettingsLoadCauseError, SettingsLoadError};
pub use settings_resolve::{
    filter_escalating_default_mode, resolve_python_settings, resolve_python_settings_with,
    resolve_python_settings_with_state, resolve_settings, resolve_settings_exact, resolve_settings_exact_in_dialect,
    resolve_settings_exact_in_dialect_with, resolve_settings_exact_in_dialect_with_state, resolve_settings_exact_with,
    resolve_settings_exact_with_state, resolve_settings_in_dialect, resolve_settings_in_dialect_with,
    resolve_settings_in_dialect_with_state, resolve_settings_json_with, resolve_settings_json_with_state,
    resolve_settings_with, resolve_settings_with_state,
};
pub use settings_resolve_error::ResolveSettingsError;
pub use settings_resolve_reread::PolicyRereads;
pub use settings_resolve_state::ResolverState;
pub use settings_sandbox::{
    SandboxCredentialsConfig, SandboxFilesystemConfig, SandboxIgnoreViolations, SandboxNetworkConfig, SandboxSettings,
};
pub use settings_sandbox_schema::{SandboxConfigRejectedError, SandboxConfigSchema};
pub use sse_ccr_event::SseCcrEvent;
pub use sse_delivery_update::SseDeliveryUpdate;
pub use sse_drop_category::SseDropCategory;
pub use sse_error::SseError;
pub use sse_frame::SseFrame;
pub use sse_frame_filter::{SseEvent, SseFrameFilter};
pub use sse_http_error::{SseHttpError, http_error_reason};
pub use sse_log_record::{SseLogLevel, SseLogRecord};
pub use sse_options::{CatchUpTruncatedHandler, DeliveryUpdateHandler, SseOptions};
pub use sse_parser::SseParser;
pub use sse_prepared_write::SsePreparedWrite;
pub use sse_transport::{G4_TEXT_ENVELOPE_ARM_PORTED, SseHandle, SseTransport};
pub use startup_timeout::StartupHooksRunning;
pub use stderr_tail::StderrTail;
pub use summary_dialect::SummaryDialect;
pub use system_prompt::{CustomPrompt, PresetPrompt};
pub use task_id::TaskId;
pub use timeout::{Timeout, TimeoutError};
pub use tokio_util::sync::CancellationToken;
pub use tool_config::{AskUserQuestionConfig, QuestionPreviewFormat, ToolConfig};
pub use tool_name::ToolName;
pub use tool_rule::ToolRule;
pub use tool_use_id::ToolUseId;
pub use transcript_mirror::{MirrorEnqueueError, MirrorFailureError, MirrorFlush, TranscriptMirror};
pub use transport::Transport;
pub use typed_mcp_tool_handler::{TypedMcpToolHandler, TypedMcpToolRequest};
pub use ui_host::{UiFuture, UiHandler, UiHost, UiRequest};
pub use upstream::hooks::{EXIT_REASONS, ExitReason, HOOK_EVENTS, HookInput, HookJSONOutput};
pub use upstream::settings::Settings;
pub use upstream::settings_resolution::{
    PolicySettingsOrigin, ProvenanceEntry, ResolveSettingsOptions, ResolvedSettingSource, ResolvedSettings,
    SDKSettingsParseError,
};
pub use urlopen_retriever::UrlopenRetriever;
pub use urlopen_retriever_options::UrlopenRetrieverOptions;
pub use usd_budget::{UsdBudget, UsdBudgetError};
pub use user_input::{InputBlock, UserInput};
pub use user_input_priority::UserInputPriority;
pub use validation_choices::{ValidationChoices, ValidationChoicesError, ValidationChoicesOptions};
pub use version_check::{parse_version, unreported_version_warning};
pub use websocket_error::WebSocketError;
pub use websocket_transport::{AuthMessage, HeaderDelivery, OAuthCredential, WebSocketOptions, WebSocketTransport};
pub use workspace_trust::WorkspaceTrust;

/// A JSON object whose schema is owned by an individual tool or upstream event.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;

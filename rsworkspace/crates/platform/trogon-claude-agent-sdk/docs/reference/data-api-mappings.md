# Data API mappings

These references connect Rust data declarations to the reviewed Python and
TypeScript SDK declarations. Rust wrappers and representation choices are
identified separately from upstream class names. Declaration comparisons and
fixture tests do not establish native CLI or model-session conformance.

Python references are pinned to commit
`2209f2c134fee914c0f75733b631a5fc238ca846`. TypeScript references use the
integrity-verified published `0.3.319` declaration archive.

| Rust field | Python field | TypeScript option |
| --- | --- | --- |
| `cli_path: Option<PathBuf>` | [`cli_path`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2467) | `pathToClaudeCodeExecutable` |
| `cwd: Option<PathBuf>` | [`cwd`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2464) | `cwd` |
| `model: Option<ModelName>` | [`model`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2438) | `model` |
| `permission_mode: Option<PermissionMode>` | [`permission_mode`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2393) | `permissionMode` |
| `setting_sources: Option<Vec<SettingSource>>` | [`setting_sources`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2666) | `settingSources` |
| `max_turns: Option<NonZeroUsize>` | [`max_turns`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2418) | `maxTurns` |
| `max_budget_usd: Option<UsdBudget>` | [`max_budget_usd`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2424) | `maxBudgetUsd` |
| `effort: Option<EffortLevel>` | [`effort`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2741) | `effort` |
| `env: Environment` | [`env`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2492) | `env` |
| `additional_directories: Vec<PathBuf>` | [`add_dirs`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2486) | `additionalDirectories` |
| `agent: Option<AgentName>` | Not exposed in pinned Python | `agent` |
| `allowed_tools: Vec<ToolRule>` | [`allowed_tools`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2353) | `allowedTools` |
| `disallowed_tools: Vec<ToolRule>` | [`disallowed_tools`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2431) | `disallowedTools` |
| `resume: Option<SessionId>` | [`resume`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2408) | `resume` |
| `thinking: Option<ThinkingConfig>` | `thinking` | `thinking` |
| `max_thinking_tokens: Option<u64>` | `max_thinking_tokens` | `maxThinkingTokens` |
| `task_budget: Option<TaskBudget>` | `task_budget` | `taskBudget` |
| `betas: Vec<SdkBeta>` | `betas` | `betas` |
| `fallback_model: Option<ModelName>` | `fallback_model` | `fallbackModel` |
| `permission_prompt_tool_name: Option<String>` | `permission_prompt_tool_name` | `permissionPromptToolName` |
| `permission_prompts: Option<PermissionPrompts>` | Not exposed in pinned Python | `permissionPrompts` |
| `channels: Vec<String>` | Not exposed in pinned Python | CLI bundle only |
| `debug: DebugOutput` | Not exposed in pinned Python | `debug` |
| `debug_file: Option<PathBuf>` | Not exposed in pinned Python | `debugFile` |
| `include_hook_events: HookEvents` | `include_hook_events` | `includeHookEvents` |
| `session_id: Option<SessionId>` | `session_id` | `sessionId` |
| `resume_session_at: Option<String>` | `resume_session_at` | `resumeSessionAt` |
| `resume_drops_turn: Option<String>` | `resume_drops_turn` | `resumeDropsTurn` |
| `allow_dangerously_skip_permissions: DangerousPermissionSkip` | Not exposed in pinned Python | `allowDangerouslySkipPermissions` |
| `project_config_root: Option<PathBuf>` | Not exposed in pinned Python | `projectConfigRoot` |
| `skills: Option<Skills>` | `skills` | `skills` |
| `mcp_servers: McpServers` | [`mcp_servers`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2378) dict | `mcpServers` entries without an `instance`, in insertion order |
| `sdk_mcp_servers: SdkMcpServers` | `mcp_servers` entries from `create_sdk_mcp_server` | `mcpServers` entries with an `instance` |
| `mcp_config: Option<OsString>` | `mcp_servers` as a `str` or `Path` | Not exposed |

| Rust value | Upstream scalar or purpose | Value contract |
| --- | --- | --- |
| `Prompt` | [Python `query(prompt=...)`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/query.py#L11) / TypeScript `query` string prompt | Infallible opaque text; every string is stored verbatim. |
| `SessionId` | [Python `ResultMessage.session_id`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1714) / TypeScript `SDKResultSuccess.session_id` | Infallible opaque text; every string is stored verbatim. |
| `MessageId` | [Python `UserMessage.uuid`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1378) / TypeScript `SDKUserMessage.uuid`; Rust does not require UUID syntax | Infallible opaque text; every string is stored verbatim. |
| `ToolUseId` | [Python `ToolUseBlock.id`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1223) / TypeScript `UserDialogRequest.toolUseID` | Infallible opaque text; every string is stored verbatim. |
| `ControlRequestId` | [Python protocol `SDKControlRequest.request_id`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2885); an internal control-envelope string, not a published TypeScript wrapper class | Infallible opaque text; every string is stored verbatim. Outbound ids follow the dialect: TypeScript `Math.random().toString(36).substring(2,15)`, Python `req_{counter}_{os.urandom(4).hex()}`. |
| `TaskId` | [Python `TaskStartedMessage.task_id`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1568) / TypeScript `SDKTaskStartedMessage.task_id` | Infallible opaque text; every string is stored verbatim. |
| `ToolName` | [Python `ToolUseBlock.name`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1224); the tool-call name string | Infallible opaque text; every string is stored verbatim. |
| `BuiltinToolName` | [Python `ClaudeAgentOptions.tools`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2342) / TypeScript `Options.tools` string-list selection | Infallible opaque text. Names are joined into the `--tools` argument unvalidated, as both upstream SDKs do; callers must avoid commas and the CLI validates each name. |
| `McpServerName` | [Python `ClaudeAgentOptions.mcp_servers`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2378) / TypeScript `Options.mcpServers` map key | Infallible opaque text; every string is stored verbatim. |
| `McpUrl` | Python / TypeScript MCP `url` string | Infallible opaque text sent verbatim, as both SDKs do. `McpUrl::validated` is an opt-in check for HTTP(S), userinfo and fragments that neither SDK performs. |
| `McpServerOptions` | Python `McpStdioServerConfig`, `McpSSEServerConfig`, `McpHttpServerConfig`, `McpSdkServerConfig` / TypeScript `McpServerConfigForProcessTransport` | Every upstream field in declaration order, then `extra` keys verbatim. `McpServerToolPolicy` carries the per-tool `tools` entries. |
| `DialogKind` | TypeScript `Options.supportedDialogKinds` entry; no corresponding option in pinned Python | Infallible opaque text; every string is stored verbatim. |
| `McpToolName` | [Python `SdkMcpTool.name`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/__init__.py#L270) / TypeScript `SdkMcpToolDefinition.name` | Infallible opaque text. The built-in MCP server warns about a name outside one to 128 ASCII letters, digits, underscores, hyphens or periods, and still lists it. |
| `ToolRule` | Python `allowed_tools` / `disallowed_tools` entry; TypeScript `allowedTools` / `disallowedTools` entry | Infallible opaque text. Rules are joined with commas unvalidated, as both upstream SDKs do; callers must avoid commas, which the CLI would split into separate rules. |
| `AgentName` | TypeScript `Options.agent`; not exposed in pinned Python | Infallible opaque text; every string is stored verbatim. |
| `QueueCapacity` | Rust host-side bounded-channel policy; not an upstream SDK class | Positive and no larger than Tokio's `Semaphore::MAX_PERMITS`; the value creates no channel. Only opted-in bounds use it; queues are unbounded by default. |
| `Timeout` | Rust host-side wait policy; not an upstream SDK class | Any duration; `Timeout::checked` rejects zero and values past 24 hours. A deadline past the clock's range never fires. No clock is read. |
| `StderrTail` | Rust diagnostic byte container for child-process failures; not an upstream SDK class | Read-only public bytes and a content-free debug representation. Only the empty default is directly publicly constructible. |
| `SessionSummaryEntry` | Python `SessionSummaryEntry` / TypeScript `SessionSummaryEntry`; `modified_at_ms` is upstream `mtime` | `data` is an opaque JSON object persisted verbatim; the fold never touches the write time unless one is passed. |
| `SummaryDialect` | Rust selector for the two upstream fold layouts | `TypeScript` (default) stores camelCase `data` keys, honors `relocated` entries and shows bash input as `! command`; `Python` stores snake_case keys. |
| `SdkSessionInfo` | Python `SDKSessionInfo` / TypeScript `SDKSessionInfo` | `file_size` is empty for store-backed listings; `created_at` is milliseconds since the Unix epoch. |
| `SessionFiles` | Python `list_sessions`, `get_session_info`, `get_session_messages`, `list_subagents`, `get_subagent_messages` / the TypeScript functions of the same names | Reads transcripts under the Claude config directory with Python semantics (64 KiB head/tail lite reads, parent-chain walk, parallel tool-result recovery) and the TypeScript newest-first tie-break by session id. `MessagePage` treats a zero or absent limit as unlimited. |
| `project_key_for_directory` | Python `_sanitize_path` / TypeScript `sanitizePath` | Realpath and NFC normalization, hyphen sanitization, and the base36 hash suffix beyond 200 characters; TypeScript hashes UTF-16 units. |
| `SessionMutationError` | Python `ValueError` / `FileNotFoundError` raised by `rename_session`, `tag_session`, `delete_session`, `fork_session` | Distinguishes invalid ids, empty title or tag, a missing session or message, and I/O failure. |
| `ForkSessionResult` | Python `ForkSessionResult` / TypeScript `ForkSessionResult` | Holds the new session id; `SessionFiles::fork_session` writes the copy beside its source with owner-only permissions on Unix. |
| `list_sessions_from_store` and the other `*_from_store` readers | Python / TypeScript store-backed session helpers | Async free functions over any `SessionStore`. Listing uses summary sidecars with gap-fill from `load` and otherwise loads each session sixteen at a time; subagent helpers need `list_subkeys`. Project keys use the TypeScript sanitization the mirror uses. `StoreListOptions` carries the summary dialect. |
| `rename_session_via_store`, `tag_session_via_store`, `delete_session_via_store`, `fork_session_via_store` | Python / TypeScript store mutations | Append `custom-title` and `tag` entries or a forked transcript; delete is a no-op for stores without `delete`. |
| `import_session_to_store`, `ImportOptions` | Python `import_session_to_store` / TypeScript `importSessionToStore` | Replays the main transcript, subagent transcripts and `.meta.json` sidecars in batches of 500 entries or 1 MiB, keyed by the on-disk project directory name. |
| `SessionStoreHelperError` | Python `ValueError` / `FileNotFoundError` plus store failures | Wraps `SessionMutationError`, `SessionStoreError`, key errors, unsupported-store capability and import I/O or JSON failures. |
| `SessionStoreFlush` | Python `session_store_flush` / TypeScript `sessionStoreFlush` | `Batched` (default) flushes on a result or past the pending limits; `Eager` appends every frame as it is read. |
| `MirrorSizeEstimate` | Pending-frame size heuristic of each upstream batcher | `TypeScript` (default) counts UTF-16 units of compact JSON; `Python` counts `json.dumps` characters with ASCII escaping. |

## Process launch options

| Rust field | Python equivalent | TypeScript equivalent |
| --- | --- | --- |
| `argv_style: ArgvStyle` | `subprocess_cli.py` argv shape | `sdk.mjs` argv shape (default) |
| `environment_mode: EnvironmentMode` | env overlay (`Overlay`) | `env` replace/inherit (`TypeScript`, default) |
| `claude_code_marker: ClaudeCodeMarker` | removes `CLAUDECODE` | keeps inherited value |
| `entrypoint: Option<String>` | `CLAUDE_CODE_ENTRYPOINT` | `CLAUDE_CODE_ENTRYPOINT` |
| `client_app: Option<String>` | `CLAUDE_AGENT_SDK_CLIENT_APP` | `CLAUDE_AGENT_SDK_CLIENT_APP` |
| `client_app_derivation: ClientAppDerivation` | `CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP` (bundled CLI at 999.0.0 or later; this crate's identity replaces the distribution metadata) | none |
| `trace_context: Option<TraceContext>` | `TRACEPARENT`, `TRACESTATE` | `TRACEPARENT`, `TRACESTATE` |
| `stderr_handler: Option<StderrHandler>` | `stderr` callback (lines) | `stderr` callback (chunks) |
| `extra_args: Vec<ExtraArgument>` | `extra_args` | `extraArgs` |
| `script_runtime: ScriptRuntime` | not applicable | `executable`, `executableArgs` (a native path runs directly with the arguments first; a path ending in `.js`, `.mjs`, `.ts`, `.tsx` or `.jsx`, matched case-sensitively, runs under the executable) |
| `oauth_token_handler: Option<Arc<dyn OAuthTokenHandler>>` | not applicable | `getOAuthToken` (with `onDecline` as `OAuthTokenResponse::decline_reason`); sets `CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH` |
| `host_auth_token_handler: Option<Arc<dyn HostAuthTokenHandler>>` | not applicable | `getHostAuthToken`; sets `CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH`. A handler returns a string or none, never the bare number the bundle would forward. Both token callbacks use `limits.callback_timeout` when that opt-in limit is set. |
| `tool_config: ToolConfig` | not applicable | `toolConfig.askUserQuestion` as `CLAUDE_CODE_QUESTION_*` variables; ignored under the Python dialect |
| `workload: Option<String>` | not applicable | `workload`, merged into the extra arguments in place; an empty value is skipped |
| `SessionStoreOptions::load_timeout`, `dialect` | `load_timeout_ms` (resume materialization wording) | `loadTimeoutMs` (default wording) |
| `ClaudeSdkClient::connect_with_adapters` | not applicable | `spawnClaudeCodeProcess`: the spawner receives the program, arguments, directory and environment overrides; the CLI is never discovered |
| `user: Option<String>` | `user` | not applicable |
| `run_as_uid: Option<u32>` | `user` as an int | `spawn` `uid` |
| `Error::CliNotFoundAt(path)` | `CLINotFoundError` | not applicable |
| `Error::WorkingDirectoryMissing(path)` | `CLIConnectionError` (cwd) | not applicable |
| `Error::Process { .. }`, `exit_code()`, `typescript_message()` | `ProcessError` | exit-code error text |
| `Error::Result(ResultError)` | `ResultError` | error result text |
| `Error::Aborted(AbortError)`, `abort()` | not applicable | `AbortError` |
| `Error::NotReadyForWriting` | `CLIConnectionError` (not ready for writing) | same text |
| `Error::WriteToTerminatedProcess { exit_code }` | `CLIConnectionError` (terminated process) | same text without the code |
| `Error::WriteFailed(io::Error)` | `CLIConnectionError` (write failed) | same text |
| `Error::JsonDecode(JsonDecodeError)`, `JsonDecodeSourceError` | `CLIJSONDecodeError` (`line`, `original_error`: a `JSONDecodeError`, or the `ValueError("Buffer size L exceeds limit N")` of the `max_buffer_size` guard) | not applicable (line skipped, no line limit) |
| `ClaudeAgentOptions::plugins`, `LocalPlugin` | `plugins` (`SdkPluginConfig`) | `plugins` (`SdkPluginConfig`, `skipMcpDiscovery`) |
| `ClaudeAgentOptions::plugin_delivery`, `PluginDelivery` | not applicable | `pluginDelivery` (`argv` or `initialize`) |
| `ClaudeAgentOptions::system_prompt`, `SystemPrompt`, `CustomPrompt`, `PresetPrompt` | `system_prompt` as `--system-prompt`, `--system-prompt-file` or `--append-system-prompt`; `excludeDynamicSections` and `systemPromptSnapshot` in `initialize` | `systemPrompt` in `initialize` (`systemPrompt` list, `appendSystemPrompt`, `systemPromptSnapshot`, `excludeDynamicSections`); no prompt flag |
| `ClaudeAgentOptions::plan_mode_instructions`, `append_subagent_system_prompt`, `tool_aliases`, `code_running_mcp_tools`, `title`, `web_search_isolation_exempt_mcp_servers` | not applicable | `planModeInstructions`, `appendSubagentSystemPrompt`, `toolAliases`, `codeRunningMcpTools`, `title`, `webSearchIsolationExemptMcpServers` in `initialize` |
| `ClaudeAgentOptions::prompt_suggestions`, `agent_progress_summaries`, `per_task_stop_affordance`, `rapid_followup_preempt`, `forward_nested_subagent_tool_calls` | not applicable | same names in `initialize`; `agentProgressSummaries` also deletes an inherited `CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES` in `EnvironmentMode::Inherit` |
| `ClaudeAgentOptions::forward_subagent_text` | `forward_subagent_text` (sent only when true) | `forwardSubagentText` (`false` is sent) |
| `ClaudeAgentOptions::workspace_trust`, `WorkspaceTrust` | not applicable | `workspaceTrust`; an accepted trust without an absolute directory is dropped with a warning |
| `ClaudeAgentOptions::capture_sdk_mcp_manifests`, `captures_sdk_mcp_manifests` | not applicable | `captureSdkMcpManifests`, default off when `CLAUDE_AGENT_SDK_DISABLE_MCP_MANIFESTS` is truthy; gates a manifest capture this crate does not do yet |
| `ClaudeAgentOptions::verbatim_prompts`, `VerbatimPrompts` | `verbatim_prompts` (`client_composed` on each user message; warns below CLI 2.1.248) | `verbatimPrompts` (`client_composed`) |
| `Error::MessageParse(MessageParseError)` | `MessageParseError` (`data`) | not applicable |
| `Transport` | `Transport` | not applicable |
| `Transport::read_message` | `Transport.read_messages` | not applicable |
| `Transport::kill` | not applicable | abort signal |
| `ClaudeSdkClient::connect_with_transport`, `query_with_transport` | `ClaudeSDKClient(transport=...)`, `query(transport=...)` | not applicable |
| `Error::Transport(..)` | exception raised by the transport | not applicable |

## Query controls

All methods are on `ClientHandle`. Requests match `sdk.mjs` by default and
`claude_agent_sdk` under `ArgvStyle::Python`; replies are `ControlReply<T>`
(`as_json()` keeps the sent value, `None` when the reply has none where
TypeScript returns `undefined`, `decode()` reads the generated type).

| Rust | Python | TypeScript |
| --- | --- | --- |
| `initialization_result`, `reinitialize` | `get_server_info` (cached result only) | `initializationResult`, `reinitialize` |
| `supported_commands`, `supported_models`, `supported_agents`, `account_info` | not applicable | `supportedCommands` (latest `commands_changed` list first), `supportedModels`, `supportedAgents`, `accountInfo` |
| `set_model(Option<String>)` | `set_model` (`model: null` when unset) | `setModel` (`model` omitted when unset) |
| `set_mcp_permission_mode_override`, `McpPermissionModeOverride` | not applicable | `setMcpPermissionModeOverride` (`{}` for an absent response) |
| `set_max_thinking_tokens`, `set_max_thinking_tokens_with_display`, `MaxThinkingTokensDisplay` | not applicable | `setMaxThinkingTokens` |
| `apply_flag_settings`, `update_settings`, `SettingsUpdateSource` | not applicable | `applyFlagSettings`, `updateSettings` |
| `rewind_files(MessageId, Option<RewindMode>)` | `rewind_files` (no dry run) | `rewindFiles` (`dry_run`) |
| `mcp_status`, `mcp_server_status` | `get_mcp_status` | `mcpServerStatus` (`response.mcpServers`) |
| `context_usage(Option<ContextUsageDetail>)` | `get_context_usage` | `getContextUsage` |
| `usage_experimental(bool)` | not applicable | `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET` (`skip_behaviors` sent only when true) |
| `read_file`, `ReadFileOptions`, `ReadFileEncoding` | not applicable | `readFile` (`null` on any failure, here `None`) |
| `read_mcp_resource` | not applicable | `readMcpResource` |
| `reload_plugins`, `ReloadPluginsOptions`, `reload_skills`, `reload_output_styles` | not applicable | `reloadPlugins`, `reloadSkills`, `reloadOutputStyles` |
| `seed_read_state(&str, f64)` | not applicable | `seedReadState` (`mtime` spelled like `JSON.stringify`) |
| `stop_task` | `stop_task` | `stopTask` |
| `background_tasks(Option<ToolUseId>) -> bool` | not applicable | `backgroundTasks` (`true` for an absent `backgrounded`; a non-boolean is `Error::Protocol` here) |
| `Error::ControlRejected`, `ControlRejectionError::error_code` | `Exception(error)` (no code) | `error_code` kept when it matches `/^[a-z][a-z0-9_]{0,63}$/` |

## Inbound control requests

Answers match `sdk.mjs` by default and `claude_agent_sdk` under
`ArgvStyle::Python`.

| Rust | Python | TypeScript |
| --- | --- | --- |
| `ToolPermissionRequest` | `ToolPermissionContext` (suggestions, tool use, agent, blocked path, decision reason, title, display name, description) | `canUseTool` options; `decisionReasonType`, `decisionReasonCode`, `serverPolicyAskOnly`, `plugin`, `serverPrompt`, `computerFolder` and `matchedAskRule` only when well typed, otherwise kept in `additional_fields` |
| `PermissionDecision::Allow` | `PermissionResultAllow` (`updatedInput` falls back to the original input) | result written as returned with `toolUseID` appended |
| `PermissionDecision::Deny`, `DenialDisposition` | `PermissionResultDeny` (`interrupt` only when true) | result written as returned with `toolUseID` appended |
| `PermissionDecisionClassification` | not applicable | `decisionClassification` |
| `PermissionDecision::AnsweredOutOfBand` | `TypeError` error response | `null`: no response |
| `PermissionUpdate` in `updatedPermissions` | `PermissionUpdate.to_dict` (`type`, `destination`, then the fields); an update without a destination is a `PermissionUpdate::Unknown` raw object | the update in declaration order |

## Messages

| Rust | Python | TypeScript |
| --- | --- | --- |
| `Message` (`raw()`, `to_sdk_message()`) | `Message` union | `SDKMessage` (generated as `upstream::messages::SDKMessage`, every member plus `Unknown`) |
| `MessageDecoding::{TypeScript, Python}`, `ClaudeAgentOptions::message_decoding` | `parse_message` | `query()` message stream |
| `Message::RateLimit(RateLimitEvent)`, `RateLimitInfo` | `RateLimitEvent`, `RateLimitInfo` | `SDKRateLimitEvent` |
| `Message::ConversationReset(ConversationResetMessage)` | `ConversationResetMessage` | `SDKConversationResetMessage` |
| `SystemEvent::{TaskStarted, TaskProgress, TaskNotification, TaskUpdated, MirrorError, Hook}` | `TaskStartedMessage` and the other `SystemMessage` subclasses | `SDKTaskStartedMessage` and the other system members |
| `ResultMessage::deferred_tool()` | `DeferredToolUse` | `SDKResultSuccess.deferred_tool_use` |
| `ContentBlock::{ServerToolUse, ServerToolResult}` | `ServerToolUseBlock`, `ServerToolResultBlock` | server tool blocks of `BetaMessage` |
| `Error::MessageParse(MessageParseError)` with Python decoding | `MessageParseError` (`KeyError` text) | not applicable |

## Hooks

| Rust | Python | TypeScript |
| --- | --- | --- |
| `HookRegistration` (`matcher`, `callbacks`, `timeout`, `on_failure`) | `HookMatcher` | `HookCallbackMatcher` |
| `HookRequest` (`input`, `tool_use_id`, `signal`) | callback arguments and `HookContext.signal` | `HookCallback` arguments and `{ signal }` |
| `HookRequest::typed_input()` returning `HookInput` (generated, every event plus `Unknown`) | `HookInput` and its `*HookInput` TypedDicts | `HookInput` and its `*HookInput` members |
| `HookOutput::Json(HookJSONOutput)` (generated, sent exactly as given) | `HookJSONOutput`, `SyncHookJSONOutput`, `AsyncHookJSONOutput` (`continue_` and `async_` renamed on the wire) | `HookJSONOutput`, `SyncHookJSONOutput`, `AsyncHookJSONOutput` |
| `HookOutput::{Sync, Async}`, `HookOutput::block` | `SyncHookJSONOutput`, `AsyncHookJSONOutput` (common fields only) | same |
| `HookEvent`, `HookEvent::ALL`, `HOOK_EVENTS` | `HookEvent` | `HookEvent`, `HOOK_EVENTS` |
| `ExitReason`, `EXIT_REASONS` | no type | `ExitReason`, `EXIT_REASONS` |

## Settings

| Rust | Python | TypeScript |
| --- | --- | --- |
| `Settings` (generated, all 182 declared keys plus unknown keys) | no type; the `settings` option is a string | `Settings` |
| `SettingsInput::{Path, Json, Inline}` | `settings` string, path or JSON by braces | `Options.settings: string \| Settings` |
| `ClaudeAgentOptions::managed_settings`, `ManagedSettingsDelivery::{Argv, Initialize}` | none (ignored) | `Options.managedSettings`, `managedSettingsDelivery` |
| `SandboxSettings`, `SandboxNetworkConfig`, `SandboxFilesystemConfig`, `SandboxCredentialsConfig`, `SandboxIgnoreViolations` (aliases of the generated `Settings.sandbox` types) | `SandboxSettings`, `SandboxNetworkConfig`, `SandboxIgnoreViolations` TypedDicts (subset of the keys) | `SandboxSettings` and its parts |
| `ResolvedSettings`, `ProvenanceEntry`, `ResolvedSettingSource`, `PolicySettingsOrigin`, `ResolveSettingsOptions`, `SDKSettingsParseError` | no type | same names |
| `resolve_settings`, `resolve_settings_with`, `filter_escalating_default_mode` | no function | `resolveSettings`, `filterEscalatingDefaultMode` |

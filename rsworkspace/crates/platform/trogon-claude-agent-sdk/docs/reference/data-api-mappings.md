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

| Rust value | Upstream scalar or purpose | Value contract |
| --- | --- | --- |
| `Prompt` | [Python `query(prompt=...)`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/query.py#L11) / TypeScript `query` string prompt | Infallible opaque text; every string is stored verbatim. |
| `SessionId` | [Python `ResultMessage.session_id`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1714) / TypeScript `SDKResultSuccess.session_id` | Infallible opaque text; every string is stored verbatim. |
| `MessageId` | [Python `UserMessage.uuid`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1378) / TypeScript `SDKUserMessage.uuid`; Rust does not require UUID syntax | Infallible opaque text; every string is stored verbatim. |
| `ToolUseId` | [Python `ToolUseBlock.id`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1223) / TypeScript `UserDialogRequest.toolUseID` | Infallible opaque text; every string is stored verbatim. |
| `ControlRequestId` | [Python protocol `SDKControlRequest.request_id`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2885); an internal control-envelope string, not a published TypeScript wrapper class | Infallible opaque text; every string is stored verbatim. |
| `TaskId` | [Python `TaskStartedMessage.task_id`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1568) / TypeScript `SDKTaskStartedMessage.task_id` | Infallible opaque text; every string is stored verbatim. |
| `ToolName` | [Python `ToolUseBlock.name`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1224); the tool-call name string | Infallible opaque text; every string is stored verbatim. |
| `BuiltinToolName` | [Python `ClaudeAgentOptions.tools`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2342) / TypeScript `Options.tools` string-list selection | Infallible opaque text. Names are joined into the `--tools` argument unvalidated, as both upstream SDKs do; callers must avoid commas and the CLI validates each name. |
| `McpServerName` | [Python `ClaudeAgentOptions.mcp_servers`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2378) / TypeScript `Options.mcpServers` map key | Infallible opaque text; every string is stored verbatim. |
| `DialogKind` | TypeScript `Options.supportedDialogKinds` entry; no corresponding option in pinned Python | Infallible opaque text; every string is stored verbatim. |
| `McpToolName` | [Python `SdkMcpTool.name`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/__init__.py#L270) / TypeScript `SdkMcpToolDefinition.name` | Infallible opaque text. The built-in MCP server warns about a name outside one to 128 ASCII letters, digits, underscores, hyphens or periods, and still lists it. |
| `ToolRule` | Python `allowed_tools` / `disallowed_tools` entry; TypeScript `allowedTools` / `disallowedTools` entry | Infallible opaque text. Rules are joined with commas unvalidated, as both upstream SDKs do; callers must avoid commas, which the CLI would split into separate rules. |
| `AgentName` | TypeScript `Options.agent`; not exposed in pinned Python | Infallible opaque text; every string is stored verbatim. |
| `QueueCapacity` | Rust host-side bounded-channel policy; not an upstream SDK class | Positive and no larger than Tokio's `Semaphore::MAX_PERMITS`; the value creates no channel. |
| `Timeout` | Rust host-side wait policy; not an upstream SDK class | A positive duration no longer than the runtime draft's 24-hour maximum; no clock is read. |
| `StderrTail` | Rust diagnostic byte container for child-process failures; not an upstream SDK class | Read-only public bytes and a content-free debug representation. Only the empty default is directly publicly constructible. |
| `SessionSummaryEntry` | Python `SessionSummaryEntry` / TypeScript `SessionSummaryEntry`; `modified_at_ms` is upstream `mtime` | `data` is an opaque JSON object persisted verbatim; the fold never touches the write time unless one is passed. |
| `SummaryDialect` | Rust selector for the two upstream fold layouts | `TypeScript` (default) stores camelCase `data` keys, honors `relocated` entries and shows bash input as `! command`; `Python` stores snake_case keys. |
| `SdkSessionInfo` | Python `SDKSessionInfo` / TypeScript `SDKSessionInfo` | `file_size` is empty for store-backed listings; `created_at` is milliseconds since the Unix epoch. |
| `SessionFiles` | Python `list_sessions`, `get_session_info`, `get_session_messages`, `list_subagents`, `get_subagent_messages` / the TypeScript functions of the same names | Reads transcripts under the Claude config directory with Python semantics (64 KiB head/tail lite reads, parent-chain walk, parallel tool-result recovery) and the TypeScript newest-first tie-break by session id. `MessagePage` treats a zero or absent limit as unlimited. |
| `project_key_for_directory` | Python `_sanitize_path` / TypeScript `sanitizePath` | Realpath and NFC normalization, hyphen sanitization, and the base36 hash suffix beyond 200 characters; TypeScript hashes UTF-16 units. |
| `SessionStoreFlush` | Python `session_store_flush` / TypeScript `sessionStoreFlush` | `Batched` (default) flushes on a result or past the pending limits; `Eager` appends every frame as it is read. |
| `MirrorSizeEstimate` | Pending-frame size heuristic of each upstream batcher | `TypeScript` (default) counts UTF-16 units of compact JSON; `Python` counts `json.dumps` characters with ASCII escaping. |

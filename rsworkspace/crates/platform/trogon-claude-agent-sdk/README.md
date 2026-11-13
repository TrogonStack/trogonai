# Rust Claude Agent SDK

`trogon-claude-agent-sdk` launches an installed Claude Code executable and speaks
its bidirectional `stream-json` protocol. Claude Code supplies the agent loop,
model access and built-in tools; this library supplies typed messages,
callbacks, controls and session storage. A native executable needs neither
Python nor Node.js.

[The lifecycle decision](docs/explanation/lifecycle.md) explains client
ownership, query completion, hooks and cleanup, and why the Rust API combines
explicit readiness with query convenience.

## Use the library

The crate is a workspace member available as a path dependency. It is not
published to crates.io.

```toml
[dependencies]
trogon-claude-agent-sdk = { path = "path/to/trogon-claude-agent-sdk" }
tokio = { version = "1", features = ["macros", "rt"] }
```

```rust,no_run
use trogon_claude_agent_sdk::{ClaudeAgentOptions, Message, Prompt, query};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut response = query(
        Prompt::from("Explain this project without changing files."),
        ClaudeAgentOptions::default(),
    ).await?;
    while let Some(message) = response.next_message().await? {
        if let Message::Result(result) = message {
            println!("{}", result.result.unwrap_or_default());
        }
    }
    Ok(())
}
```

Queries can consume model usage and execute tools according to CLI permissions.
[The connect example](examples/connect.rs) initializes and closes without a
prompt. [The query example](examples/query.rs) prints assistant text.

## Turns, runs and input

`query()` receives the run through completion, including background-agent
continuation turns. `query_turn()` explicitly stops after the first result.
A result marks a turn boundary, which may precede run completion. The driver
observes session state and delegated-agent lifecycle, with a bounded
between-turn wait. Older CLIs that omit session state provide weaker completion
information.

For interactive use, create a `ClaudeSdkClient`, send a prompt and receive
through `Message::Result`. The next prompt reuses the process.
`send_input(UserInput)` accepts structured content and message identity.
`InputBlock` is a transparent JSON-object wrapper forwarded verbatim, with a
`text` convenience constructor; the CLI and the Messages API validate block shapes.

Ordinary sends reject overlapping turns. Use the cloneable `ClientHandle` for
queued input or `stream_input`, and receive on the owner concurrently. Call
`finish_input()` when no more user messages will be sent. Reported queue counts
handle CLIs that coalesce queued sends into a turn.

Call `disconnect().await` to wait for shutdown and receive cleanup errors. Dropping
the owner requests shutdown even when a handle survives, but does not call or
await `disconnect()`. Cleanup needs a running Tokio runtime; the child also has a
`kill_on_drop` fallback.

## Messages and controls

Every message keeps the object the CLI wrote (`Message::raw`), and
`Message::to_sdk_message` types it as the TypeScript `SDKMessage` generated from
the pinned declarations. `message_decoding` picks the upstream decoder:
`MessageDecoding::TypeScript` (the default) yields every message, unknown types
and shapes Rust cannot type included as `Message::Unknown`; `MessageDecoding::Python`
skips unknown types and raises `Error::MessageParse` with Python's text for a known
message missing a key. Goldens from both pinned SDKs pin the two modes; see
[Message data](docs/reference/message-data.md#decoding).

`Message::Result` and `HookOutput::Sync` box their payloads to keep enum values
compact when another dependency enables `serde_json`'s `preserve_order` feature.
Pattern matching exposes the same fields through the box.

`connect().await` completes the CLI initialization handshake before returning.
`connection_info()` borrows the cached `ConnectionInfo` from that handshake,
including support flags and additional metadata. Reading it is non-fallible and
sends no protocol messages; `ConnectionInfo::as_json()` retains the original
response, including omitted or explicitly null fields. Like both upstream SDKs,
the response is not validated: a known field of an unexpected type reads as
absent, and under `ArgvStyle::Python` a response that is not an object reads as
`{}`. The initialize checks follow `sdk.mjs` in its order: blocking hooks need
`hook_on_failure_supported`, then `hooks_applied` must not be `false`, then sent
managed settings need `managed_settings_applied`; plugins that were not
confirmed only log a warning. Non-empty `supported_dialog_kinds` without a
`user_dialog_handler` is refused before the CLI starts.
The handle provides interrupt receipts, model and permission changes, rewind,
MCP status/reconnect/toggle/replacement, context usage and task controls. Legacy
interrupt replies remain distinct from reported empty queues. Newer controls
depend on installed CLI support.

## Callbacks and MCP

`ToolPermissionHandler` receives request/tool identities, suggestions, MCP
provenance and prompt context, plus a cancellation signal. Typed allow decisions
can replace input and update permission rules. Absent handlers deny host
permission requests; an opted-in `limits.callback_timeout` denies, and cancellation
suppresses a late reply.
The CLI can authorize tools through its own permission mode or rules before
consulting the host.
Suggestions keep unknown destinations, behaviors and update types verbatim, so
they round-trip into an allow decision. A permission request that cannot be
decoded gets an error response and the session continues.

`HookRegistration` and `HookOutput` support hook callbacks. Blocking failure
semantics require advertised CLI support and registered hooks; startup refuses
incompatible responders. [The hooks example](examples/hooks.rs) blocks shell
commands. Elicitation and user-dialog callbacks also receive cancellation
signals. Declare supported dialog kinds explicitly.

[The feature-connect example](examples/feature_connect.rs) checks blocking-hook
registration and MCP connection without a prompt. It refuses startup when the
installed CLI does not advertise blocking-hook support. Verification with CLI
`2.1.312` exercised that refusal; the pinned CLI `2.1.319` accepted registration
and connected MCP. Pass an executable path as the example's first argument to
check a separate CLI installation.

Build a `LocalMcpServer` with named `LocalMcpServerOptions` fields. Define each
schema-validated tool through `SdkTool::from(SdkToolOptions { ... })`, naming its
metadata, input schema and async handler. Construct its `McpToolName` and
`McpToolInputSchema` first. `McpToolName` stores any text; `LocalMcpServer::from`
logs a warning for a name outside the MCP protocol limits (one to 128 ASCII
letters, digits, underscores, hyphens or periods), and `tools/list` still lists it.
`McpToolInputSchema::new` keeps any schema verbatim, as both upstream SDKs do;
a call to a tool whose schema does not compile returns that error as the tool
result. `McpToolInputSchema::validated` opts in to requiring a compiling object
schema without external references. `SdkTool::from` then assembles its inputs
without returning an error.
`LocalMcpServerOptions.tools` requires an immutable `SdkTools` collection.
Use `tool.into()` for one tool, `SdkTools::default()` for none, or
`SdkTools::new([first, second])?` to reject duplicate names when assembling
several tools, as the TypeScript SDK does. Exact names must be unique;
case-distinct names remain separate. `SdkTools::with_duplicates` keeps repeated
names, as the Python SDK does. `LocalMcpServerOptions::new(name)` starts from no
tools; `version`, `instructions` and `always_load` mirror the upstream server
options. `LocalMcpServer::from` assembles these inputs infallibly.
Each client opens its own session. The built-in server supports initialize,
initialized notification, cancellation, ping and tool listing/calls.
`McpServerDialect::TypeScript`, the default, answers like `createSdkMcpServer`:
it drops messages it cannot read, reports parameter errors as `-32603` and
lists each schema with its draft-07 `$schema`. `McpServerDialect::Python`
answers like `create_sdk_mcp_server`: it reports unreadable messages as
`McpServerError::Protocol`, refuses requests before `initialize` and answers a
cancelled call with `-32800`. Custom
`SdkMcpServer`/`SdkMcpSession` implementations can add methods; the built-in
server does not expose server-initiated client requests.
[The SDK tools example](examples/sdk_tools.rs) registers an echo tool.
[The MCP-connect example](examples/mcp_connect.rs) checks the embedded server
connection without registering hooks or sending a prompt.

`McpServerOptions` configures the stdio, SSE, HTTP and instance-less `sdk`
servers whose connections the CLI owns. Every upstream field is typed, and
`extra` keys follow them verbatim. `McpServers` keeps insertion order, so the
`--mcp-config` JSON matches what TypeScript builds from the same object. URLs
are sent as given, like both SDKs; `McpUrl::validated` is an opt-in check for
HTTP(S), userinfo and fragments.

With the default TypeScript style, in-process servers stay out of
`--mcp-config` and are named in the `initialize` request. With
`ArgvStyle::Python` they appear in `--mcp-config` as `sdk` entries, and
`ClaudeAgentOptions.mcp_config` carries Python's file path or JSON text form
verbatim. An in-process server replaces an external config of the same name.

`mcp_message` control requests are routed the way the selected style's SDK
routes them. TypeScript fails the request for an unknown server or a message
that is not an object, waits for the server's response to a request with an
id, and acknowledges anything else with an id 0 result. Python fails the
request when the server name or message is missing, and answers an unknown
server or an undeliverable message with a JSON-RPC error under the message's
id.

`set_mcp_servers` is not serialized, as in TypeScript. It swaps in-process
servers before the request is sent and keeps the session of a server that stays
registered. Remove a server and re-add it to replace its registry instance.

## Configuration and limits

The interactive client follows Python's `ClaudeAgentOptions` and `ClaudeSDKClient`
terminology, with Rust acronym casing. Its core optional fields default to `None`:

| Rust field | Python field | TypeScript option |
| --- | --- | --- |
| `cli_path: Option<PathBuf>` | [`cli_path`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2467) | `pathToClaudeCodeExecutable` |
| `cwd: Option<PathBuf>` | [`cwd`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2464) | `cwd` |
| `model: Option<ModelName>` | [`model`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2438) | `model` |
| `permission_mode: Option<PermissionMode>` | [`permission_mode`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2393) | `permissionMode` |
| `setting_sources: Option<Vec<SettingSource>>` | [`setting_sources`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2666) | `settingSources` |

`PermissionMode` mirrors Python's
[`PermissionMode`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L25)
and the exported type in
[TypeScript `0.3.319`](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.3.319.tgz).
Serde uses their literal names: `default`, `acceptEdits`, `plan`,
`bypassPermissions`, `dontAsk`, and `auto`. `None` leaves the mode unset;
`Some(PermissionMode::Default)` explicitly selects `default`. The runtime omits
the flag for `None` and sends the explicit literal for `Some`.

`SettingSource` uses the exact serde literals `user`, `project`, and `local`.
Python's pinned `setting_sources` docs and TypeScript `0.3.319`'s `Options.settingSources`
docs both describe omission as loading all CLI sources and an empty list as
disabling filesystem settings. Rust preserves `None`, `Some(vec![])`, and caller
order when building CLI arguments.

### Process launch

The launch contract is pinned by `tests/golden_argv_env.rs` (argv and environment)
and `tests/golden_initialize.rs` (the stdin `initialize` bytes, captured from both
upstream SDKs).

- Control requests follow the dialect. `ArgvStyle::TypeScript` sends
  `{"request_id":...,"type":"control_request","request":...}` with the id
  `Math.random().toString(36).substring(2,15)` produces. `ArgvStyle::Python`
  sends `{"type": "control_request", "request_id": "req_N_xxxxxxxx", ...}` and
  writes every stdin line the way `json.dumps` does (spaced separators, `\uXXXX`
  escapes for non-ASCII, Python float spelling).

- `ArgvStyle::TypeScript` (default) emits TypeScript `0.3.323` argv: `--flag=value`
  spelling, upstream flag order, and the system prompt inside the `initialize`
  request (`systemPrompt` defaults to `[""]`; a preset with an append sends
  `appendSystemPrompt`; `tests/initialize_options.rs` pins every system prompt form
  and the other initialize-only options against captured upstream bytes).
  `ArgvStyle::Python` emits Python `0.2.181` argv. The two
  upstream SDKs disagree, so both are exposed and TypeScript is the default.
- `EnvironmentMode::Overlay` (default, Python): inherited environment, then
  `options.env`. `Replace` (TypeScript with `options.env`): only `options.env`
  plus the SDK defaults. `Inherit` (TypeScript without `options.env`).
  `CLAUDECODE` is removed unless `ClaudeCodeMarker::Keep` is chosen.
  `CLAUDE_CODE_ENTRYPOINT` defaults to `sdk-trogon-rs` and is configurable.
  `CLAUDE_CODE_SDK_READS_SESSION_STATE=1` is added unless a case-insensitive match
  exists; the inherited check covers only the variable names the SDK captures.
- `McpDiscovery` defaults to `CliDefault`; `--strict-mcp-config` is emitted only for
  the explicit `Disabled` opt-in. `--permission-prompt-tool stdio` is sent only
  when a tool permission handler exists. `HookRegistration::on_failure` is an
  `Option` and the wire field is omitted when unset.
- `ClaudeSdkClient::connect` discovers the CLI in Python's order when `cli_path` is
  omitted (no bundled binary exists in this crate); a miss is `CliNotFoundError`
  with Python's hint text. `connect_with_adapters` never discovers, matching
  Python's custom transport and TypeScript's `spawnClaudeCodeProcess`: its
  spawner receives `claude` when `cli_path` is omitted.
- `Transport` is Python's transport interface: `connect`, `write`, `read_message`
  (one object per call, `None` at the end of the stream, in place of Python's
  `read_messages` iterator), `close`, `is_ready` and `end_input`, plus a provided
  `kill` that `abort()` calls before `close` (TypeScript's abort signal). It is a
  generic bound, not a trait object. `ClaudeSdkClient::connect_with_transport`,
  `connect_with_transport_and_adapters`, `query_with_transport` and
  `query_turn_with_transport` inject one; the subprocess implementation is the
  default and stays crate-private. A custom transport gets no CLI discovery, no
  argument list and no store-backed resume materialization, while the session
  store mirror still runs, as in Python. `read_message` must be cancel safe, and
  an `Err` from any method ends the run as `Error::Transport` surfaces it.
- Bundled executables are caller-driven lookups, not part of `connect`.
  `find_bundled_cli` ports Python's `<package>/_bundled/claude[.exe]` check.
  `bundled_cli_derives_client_app` and `meets_derived_client_app_minimum` port
  Python's version gate for deriving an app label (the pinned CLI version and the
  `claude.version` record beside the bundled binary must both be `999.0.0` or
  later); `connect` asks them only through the Python dialect
  `client_app_derivation` option and otherwise follows TypeScript.
  `find_native_cli` ports TypeScript's platform package selection
  (`@anthropic-ai/claude-agent-sdk-<platform>-<arch>`, musl before glibc on musl
  Linux, `-android` packages, `.exe` on win32) over a `ResolveNodeModule`
  adapter; `NodeModuleLookup` is the default and searches Node's `node_modules`,
  `NODE_PATH`, home and prefix directories. Musl comes from the compile target
  (`NativePackageTarget::host`) because a Rust process has no Node process report.
- Failures follow upstream. A missing executable is `Error::CliNotFoundAt`, a missing
  `cwd` is `Error::WorkingDirectoryMissing`, any other spawn failure is
  `Error::Spawn` ("Failed to start Claude Code: ..."). `Error::Process` displays
  Python's `ProcessError` text, exposes `exit_code()` (negated signal number on
  Unix) and `typescript_message()` for TypeScript's text. When the CLI exits
  unsuccessfully after an `is_error` result, the error is `Error::Result`
  (`ResultError`): Python's text always, TypeScript's text when it is non-empty.
  `abort()` stops the process and ends the run with `Error::Aborted`
  (`AbortError`). Closing follows `limits.shutdown`: TypeScript's ladder by
  default (stdin close, 2 seconds, SIGTERM, SIGKILL 5 seconds later without
  waiting), Python's 5/5/5 second ladder, or `Shutdown::Strict`, which reports
  `Error::ShutdownTimeout` and `Error::StderrDrainTimeout`.
- Writes fail with distinct errors in upstream's order: `Error::NotReadyForWriting`
  after stdin is closed (and, in the TypeScript style, after the process exited,
  where the exit event clears `ready`), `Error::WriteToTerminatedProcess` after an
  exit in the Python style (with the exit code), `Error::WriteFailed` for an I/O
  failure and `Error::Aborted` after `abort()`.
- Non-JSON stdout: `ArgvStyle::TypeScript` skips every undecodable line. Python
  style skips lines that do not start with `{` and fails on a corrupt line with
  `Error::JsonDecode` (the line and the original error), except that a corrupt
  unterminated tail at end of file is dropped. With `MessageDecoding::Python`, a
  known message missing a key Python reads is `Error::MessageParse`, carrying the
  raw object.
- `ArgvStyle::TypeScript` appends `--await-initialize` through extra arguments
  when `connect` resumes or continues without `cli_path` and without `sdk-url`,
  as `sdk.mjs` does for its resolved executable. Python never sends it.
- `user` runs the CLI as the named account (`Popen(user=...)`: the name is
  resolved with libc `getpwnam_r`, so NSS sources apply, an unknown name fails
  with `getpwnam(): name not found: '<name>'`, and only the uid changes, Unix
  only); `run_as_uid` is the numeric form.
- The inbound line limit `limits.max_buffer_size` is unset by default, as
  TypeScript keeps every line; `Limits::python()` sets Python's 1 MiB
  (`max_buffer_size`). A longer line, counted in characters without its
  newline, is `Error::JsonDecode` with the line `JSON message exceeded maximum
  buffer size of N bytes` and `JsonDecodeSourceError::BufferExceeded`.
- In `ArgvStyle::TypeScript`, `end_input` leaves the transport ready, so a later
  write is dropped without an error, as `endInput` does; `close` ends that.
- `plugins` and `plugin_delivery` map `plugins` and `pluginDelivery`: TypeScript
  passes `--plugin-dir=PATH` (`--plugin-dir-no-mcp=PATH` with
  `skip_mcp_discovery`) or, for `PluginDelivery::Initialize` with at least one
  plugin, `--await-initialize` and the plugins in the `initialize` request;
  Python passes `--plugin-dir PATH` and ignores the delivery.
- `CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP` is removed from the child environment.
  `ClaudeAgentOptions::client_app_derivation` (Python dialect, off by default
  like TypeScript) sets it with Python's trigger: the resolved `cli_path` is the
  executable `find_bundled_cli` finds under the `BundledCli` package root, its
  `pinned_version` and the `claude.version` file beside it are both `999.0.0`
  or later, and no explicit `CLAUDE_AGENT_SDK_CLIENT_APP`
  exists in `client_app`, `env` or the inherited environment. The value is
  `name/version` for this crate, standing in for the Python distribution
  metadata Rust does not have.
- Not expressible: the `Cannot write to terminated process` and `exited with error` write
  branches are unreachable upstream because the `exit` event clears readiness
  first. `cli_path` or `PATH` discovery is the Rust equivalent.
- Runtime options map `thinking`, `maxThinkingTokens`, `taskBudget`, `betas`,
  `fallbackModel`, `permissionPromptToolName`, `includeHookEvents`, `sessionId`,
  `resumeSessionAt`, `resumeDropsTurn` and `skills` in both dialects with the
  upstream flag order. `permission_prompts`, `channels`, `debug`, `debug_file`,
  `allow_dangerously_skip_permissions` and `project_config_root` exist only in
  TypeScript and are ignored by `ArgvStyle::Python`. In TypeScript `connect`
  also adds the derived per-process `--debug-file` when none is set. `skills`
  appends `Skill` or `Skill(name)` to the allowed tools, Python also defaults
  `--setting-sources=user,project`, and an explicit list reaches the
  `initialize` request. A callback together with `permission_prompt_tool_name`
  is refused, TypeScript refuses a fallback model equal to the model, and
  Python refuses an enabled thinking config without a budget. The shadow
  advisory for `bypassPermissions` or allowed tools is reported through
  `tracing`.
- `settings` (a path, JSON text or a `Settings` object), `sandbox`,
  `managed_settings` and `managed_settings_delivery` follow both dialects.
  TypeScript serializes objects with `JSON.stringify` bytes and merges the
  sandbox into the settings document (a settings file path together with a
  sandbox is refused); Python reads the settings file relative to the working
  directory, replaces the `sandbox` key and writes `json.dumps` text. Managed
  settings go by `--managed-settings` or, with
  `ManagedSettingsDelivery::Initialize`, in the `initialize` request; the Python
  dialect ignores them.
- Deferred to other tracks: remote `--sdk-url` transports, flag settings and
  `updateSettings`.

Model strings and paths are stored without extra identifier restrictions.
`env` values are `ConfigValue`s, which convert from strings as `Sensitive`;
debug output lists every variable name, shows values the caller marks `Plain`,
and prints sensitive values as `<redacted>`. MCP stdio arguments and
environment values and HTTP or SSE header values follow the same rule.
`ModelName` keeps the model out of debug output, so options debug output shows
every other setting without listing fields by hand.
A Windows `.bat` or `.cmd` CLI path starts, as in TypeScript;
`WindowsBatchCli::Refuse` selects Python's refusal at process start.
Python exposes [`connect`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/client.py#L93)
and [`disconnect`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/client.py#L607)
on an existing client. Rust intentionally uses an async factory for `connect`
and consumes the client in `disconnect(self)` to observe cleanup errors. Published
[TypeScript `0.3.319`](https://www.npmjs.com/package/@anthropic-ai/claude-agent-sdk/v/0.3.319)
uses `Query` and `WarmQuery` with `close()` instead.

Options include programmatic agents, model/effort, tool rules, budget, output
schema, checkpointing, environment, additional directories, resume and fork.
`OutputSchema::new` and `UsdBudget::new` keep any value and pass it on, as both
upstream SDKs do; `OutputSchema::validated` and `UsdBudget::checked` opt in to
rejecting nonlocal references and nonfinite or negative amounts. Rewind
requires checkpointing and CLI support.

The default system prompt is empty; choose `SystemPrompt::ClaudeCode` for the
CLI preset. `SystemPrompt` also carries a list of texts, the custom and preset
objects with `snapshot` and `exclude_dynamic_sections`, and Python's prompt
file; a form that only one dialect has is refused in the other. `verbatim_prompts`
marks each user message `client_composed`. Filesystem settings inherit CLI defaults when `setting_sources` is
`None`; `Some(vec![])` disables user/project/local settings. Explicit lists retain
caller order and duplicates. MCP discovery follows the CLI default, as upstream
does, unless `McpDiscovery::Disabled` is selected. `Customizations::Bare` disables further CLI
customizations when supported.

`Limits::default()` matches the TypeScript SDK: no stdout line cap, the last
2048 UTF-16 units of stderr kept for process errors, unbounded queues, no
control, write or callback timeout, and an initialize deadline of 660 seconds
only when a hook is set to block on failure. `Limits::python()` selects the
Python SDK's values: a 1 MiB line cap, a 60 second control wait, a 100 message
buffer that stops reading stdout while full, and its initialize wait and
shutdown ladder. Every other bound (pending controls, inbound callbacks, tracked
tasks, queued inputs, a failing message queue, command queue room) is opt-in and
keeps its typed error. Stderr debug output omits its contents. `QueueCapacity`
validates Tokio's range; `Timeout::checked` rejects zero and values past 24
hours.

## Settings

`Settings` is generated from the pinned TypeScript declarations: every declared key is a
typed field, and keys it does not declare are kept in order. `Settings::from_json` then
`to_json` reproduces the input byte for byte. `SettingsInput` names the three forms the
`--settings` value takes: `Path`, `Json` and `Inline(Settings)`. Nothing is validated.
`SandboxSettings` and its parts, `ResolvedSettings`, `ProvenanceEntry`, `ResolvedSettingSource`,
`PolicySettingsOrigin`, `ResolveSettingsOptions` and `SDKSettingsParseError` carry the upstream
names; Python sandbox dicts decode and encode through the sandbox types unchanged.

`resolve_settings` ports TypeScript `resolveSettings`: the user, project, local and flag sources
cascade with array union, and the managed policy comes from the first tier that has settings
(remote, macOS plist via `plutil`, Windows registry via `reg query`, `/etc/claude-code`, the
parent process). The result carries per-key provenance and `machinePolicy.didNotLoad`.
`resolve_settings_with` runs it against any `SettingsHost`, so tests inject files, variables and
command output. `filter_escalating_default_mode` ports `filterEscalatingDefaultMode`. The
`claude-sdk:resolve-diff` mise task compares the output with the pinned TypeScript resolver
byte for byte. Integers above 2^53 read like `JSON.parse` by default; `resolve_settings_in_dialect`
and `resolve_settings_in_dialect_with` take `SettingsDialect::Python` to keep them exact. Gaps: schema value normalization, and cloud and org mode.

## External session stores

Set `session_store` to a `SessionStoreOptions`. Implement `SessionStore` for a
backend or use `InMemorySessionStore`. Entries are keyed by project/session and
optional subagent path; the in-memory backend deduplicates stable UUIDs.
[The store example](examples/session_store.rs) needs no Claude or network.

Mirroring follows CLI local writes. Each result waits for its own store barrier
while controls remain serviceable. Failures appear as `mirror_error` messages
and never fail the run; a transcript path outside the projects directory is
logged and dropped. Pending entries are coalesced per key and drained early when
they pass the entry or byte budget; frames are never rejected. A failed append
is retried twice with 200 ms and 800 ms backoff, for any backend error; timeouts
are not retried because an in-flight write may still complete.

Store resume, like both upstream SDKs, loads transcripts before startup into a
private temporary directory (`claude-resume-*`, mode 0700) and points the CLI's
`CLAUDE_CONFIG_DIR` at it, so the caller's own configuration directory is never
written. Authentication is copied in: `.credentials.json` with its refresh
token removed, `.claude.json`, and `settings.json` without plugin entries or a
nested `CLAUDE_CONFIG_DIR`. On macOS, without a configured directory or an
API-key or OAuth-token variable, the keychain entry is read instead. The
directory is removed after the process is reaped, retrying briefly while it is
busy. A non-UUID, missing or empty session skips materialization and leaves
`--resume` to the CLI. Latest-session resume walks newest to oldest and skips
non-UUID, empty and sidechain sessions. File checkpointing cannot be combined
with a session store.

Project-key sanitization uses the pinned TypeScript SDK’s UTF-16 replacement
behavior. Native hashing can differ for long project paths; use an explicit
`SessionStoreOptions.project_key` when
necessary. Backend retention and cross-process coordination belong to the backend.

## Compatibility and verification

The references are Python SDK
[`0.2.179`](https://github.com/anthropics/claude-agent-sdk-python/tree/2209f2c134fee914c0f75733b631a5fc238ca846)
and published TypeScript SDK `0.3.319`, paired with CLI `2.1.319`.
This independent Rust implementation does not provide TypeScript browser/bridge
hosting, every upstream convenience option or the complete MCP server protocol.
Windows batch scripts start as in TypeScript unless `WindowsBatchCli::Refuse`
is selected.

```sh
mise exec -- cargo test --locked -p trogon-claude-agent-sdk --all-features
mise exec -- cargo clippy --locked -p trogon-claude-agent-sdk --all-targets --all-features
mise exec -- cargo fmt -p trogon-claude-agent-sdk --check
```

`test-support` enables the fixture CLI/subprocess suite. Store/type tests need
no credentials. Unix-only filesystem/reaping tests are gated. Query examples are
compiled, not executed by tests. Fixture results establish library/wire behavior,
not live model or hosted provider conformance.

[Runtime adapters](docs/explanation/runtime-adapters.md) explains how to replace
process, environment, filesystem and wall-clock effects through generic traits,
and how to use Tokio virtual time for retry and deadline tests.

Pinned declaration relationships for the extracted options, validated values and
message data are collected in [Data API mappings](docs/reference/data-api-mappings.md).

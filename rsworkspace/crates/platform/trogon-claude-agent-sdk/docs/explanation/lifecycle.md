# Client lifecycle and API design

The Rust SDK uses an asynchronously initialized process owner, a consuming
asynchronous disconnect operation, and query helpers over the same protocol runtime.
This makes readiness, ownership and cleanup visible while keeping simple
queries convenient. It is the lifecycle contract for this crate.

## Why Python and TypeScript look different

The reviewed Python SDK positions `ClaudeSDKClient` for interactive sessions,
follow-up prompts and interrupts. Its constructor stores configuration;
`await connect()` starts the transport and initializes the protocol.
`async with ClaudeSDKClient(...)` pairs connection with `disconnect()`.
Python also offers a top-level `query()` async iterator for predetermined
inputs. Both facades use bidirectional streaming internally.

The reviewed TypeScript SDK puts streaming and control methods on the `Query`
returned synchronously by `query({ prompt, options })`. Initialization proceeds
asynchronously behind that object. It also exposes `startup()`, which awaits
initialization and returns a `WarmQuery` accepting a single subsequent query.
That facade provides explicit readiness without being a permanently reusable
client.

Python's ordinary `__init__` is synchronous, so awaited startup needs another
entry point, such as `connect()` or an async context manager. TypeScript keeps
ordinary `query()` immediately iterable and carries asynchronous startup behind
the returned object. An async Rust factory can instead return `Result<Self>`
after startup, putting readiness errors at the caller's `await` boundary.
These are API choices; TypeScript's `startup()` shows that an awaited entry
point is useful there too.

These APIs emphasize different calling patterns and language ergonomics. Their
names do not establish different transports or the authors' historical motives.
Python's explicit interactive ownership and TypeScript's convenient query
object are both useful. Rust combines those roles through its own ownership
and error boundaries.

| Concern | Python | TypeScript | Rust decision |
| --- | --- | --- | --- |
| Interactive ownership | Reusable `ClaudeSDKClient` | A streaming `Query` also has controls and streamed input | Reusable `ClaudeSdkClient` owns one CLI process |
| Readiness | Await `connect()` or enter `async with` | Ordinary `query()` starts asynchronously; `startup()` awaits readiness | Await `connect()` before receiving a client |
| Simple queries | `query()` async iterator | `query()` returns an async generator with controls | `query()` returns an owned query over the same driver |
| Cleanup | Await `disconnect()` or leave the context | `Query.close()` starts cleanup; generator return/disposal can await it | Consuming `disconnect().await` observes cleanup and errors |

Rust avoids exposing a usable but disconnected client. The constructor future
can report startup errors before the caller sends input. Consuming the owner
on disconnect prevents further use through that value. The existing configuration
type is `ClaudeAgentOptions`; matching an upstream spelling is not the reason
for this lifecycle choice. The tradeoff is explicit awaiting and cleanup
ownership, in exchange for a visible readiness and failure boundary.

## From configuration to a reusable client

SDK-owned public constructor and input settings types use the `Options`
suffix consistently, whether their fields are required or have defaults.
This includes `ClaudeAgentOptions`, `SdkToolOptions`, `LocalMcpServerOptions`,
`McpServerOptions` and `SessionStoreOptions`.

Public constructors that assemble several settings use named fields.
`SdkTool::from(SdkToolOptions { ... })` identifies the tool name,
description, input schema and handler at the call site.
`LocalMcpServer::from(LocalMcpServerOptions { ... })` likewise names its server,
version and tools. These are required inputs, so a struct literal makes omitted
fields a compile-time error. The API retains this Options pattern.

Scalar text values (`Prompt`, `AgentName`, `ToolRule`, `BuiltinToolName`,
`ToolName`, `McpToolName`, `McpServerName`, `DialogKind` and the session,
message, tool-use, control-request and task identifiers) are infallible opaque
storage: construction never fails and never rewrites the text. The runtime
checks the properties that matter when it starts the process or emits a frame.
Tool lists are joined with commas and passed to the CLI unchanged, as both
upstream SDKs do; the CLI validates each name. The standard process spawn
refuses NUL bytes in arguments and environment. The built-in MCP server lists
every registered tool; an unknown tool or a failing handler returns
`{content, isError: true}` rather than a JSON-RPC error. Permission modes and
user content blocks pass through unvalidated; an unknown mode is kept in
`PermissionMode::Unknown`. Other independent input constraints belong to
value objects with private fields and typed construction errors. Its
`McpToolInputSchema` compiles the JSON schema and requires the
top-level `type` to be `object`, preserving local references and rejecting
external references. Neither value object exposes mutable access to its value.
`SdkTool::from` assembles its fields infallibly.

Duplicate names are a constraint across the collection of tools.
`LocalMcpServerOptions.tools` requires `SdkTools`, which validates exact-name
uniqueness in `SdkTools::new()` and returns a typed `SdkToolsError` identifying
the repeated `McpToolName`. Case-distinct names remain separate. Its private
immutable registry preserves that invariant when cloned; callers cannot modify
the collection after validation. Empty collections use `SdkTools::default()`
and one tool uses `tool.into()`, both infallibly. `LocalMcpServer::from` then
assembles the validated inputs infallibly.

The general `ToolName` and `OutputSchema` types retain their broader contracts
for other SDK APIs. Simple value-object constructors retain their single value.

| Phase | Rust behavior |
| --- | --- |
| Configure | Supply optional cli_path, settings, limits, hooks, permissions and MCP registrations before connecting. |
| Initialize | Prepare any store-backed resume, spawn the CLI, start the driver and exchange initialization/control messages. |
| Ready | `connect().await` returns after successful initialization and required blocking-hook checks. No user prompt is required. |
| Work | Send input and consume messages while the driver routes controls and callback requests. Reuse the process for subsequent turns. |
| Finish input | Declare that no further user messages will be sent; allow remaining turns and callback-capable work to settle. |
| Disconnect | Request shutdown, cancel callbacks, close input, reap the process and finish accepted storage work, reporting the outcome. |

Initialization may perform configuration, authentication, MCP and CLI startup
hook work. A connection without a model prompt is not a promise that startup
has no side effects. Ready means the control protocol is initialized; inspect
MCP status separately. Startup callbacks can run before `connect()` returns.
The initialization exchange uses `limits.control_timeout`; startup hooks must
complete within that configured bound. This is separate from how long a model
turn may execute.

The initialization exchange is internal to `connect()`. It configures the
protocol and checks requested blocking-hook support before returning the
client. `connection_info()` borrows the cached `ConnectionInfo` reported by that
exchange; it cannot fail or initialize the process again. The metadata is a
startup snapshot, so later control changes do not update it.
`ConnectionInfo::as_json()` preserves the original response, including unknown
fields and the distinction between omitted and explicitly null fields.

The process owner and persisted conversation identity are separate. Closing a
client ends its process. Resuming a conversation uses stored identity and
transcripts to create another client; it does not resurrect a closed owner.
`ClientHandle` permits concurrent control and input without extending the
owner's lifetime. Input sent while a turn runs is written immediately, as both
upstream SDKs do; outbound frames are not capped by `max_frame_bytes`, which
bounds inbound frames only, and only in `ArgvStyle::Python` (TypeScript keeps every line). Continue consuming messages while work runs. Queue
capacity is finite, and a lagging consumer can end the session with
`Error::MessageQueueFull`.

## Turns, runs and interrupts

A `Message::Result` is a turn boundary. Queued inputs, background agents and
follow-up turns can still belong to the run. The driver tracks reported session
state, queued input and delegated work before closing stdin for full-run
completion. Older CLIs without session-state reporting provide weaker evidence;
the bounded between-turn wait is not a universal model execution timeout. When
it expires the driver logs a warning and ends the run without an error. Non-JSON
stdout lines are skipped with a debug log, a control response for an unknown
request is ignored, and an unknown session state is kept as unknown.

Use `query()` when the caller wants the complete run. Use `query_turn()` when
stopping after the first result is intentional. For an interactive client,
receive through a result and keep the owner for later input. Exhausting a user
input stream is distinct from ending the control channel: hooks, permission
questions and embedded MCP calls may still require replies on stdin.
`stream_input()` calls `finish_input()` on exhaustion, rejecting further user
input; finishing input is not an explicit close operation.

`interrupt()` requests that the CLI interrupt current work. It does not close
the owner or necessarily discard queued input. Inspect the typed receipt and
use the explicit queued-cancellation operation when that is intended. Closing
the owner is a separate decision about the process lifetime.

## Hooks and callback round trips

Hook functions remain in the host process. Initialization sends callback IDs,
event matchers, timeouts and failure policy to the CLI. A later control request
selects a registered callback; the driver runs it asynchronously and sends a
structured response. Hooks are not application message-stream handlers.
Permission, elicitation, dialog and embedded MCP responders use the same
bidirectional control machinery.

Rust supplies per-request cancellation tokens, callback admission limits and
timeouts. Once the SDK observes control cancellation, it cancels the matching
task; identity checks suppress stale replies to reused request IDs. A timeout or
callback error produces a protocol response rather than inventing success.
The driver does not wait indefinitely for the application to consume a message
before servicing controls; overflow is an explicit terminal error. Cancellation
does not roll back an external effect or stop arbitrary blocking work.

Blocking failure policy requires advertised CLI support. When a blocking hook is
registered, startup also refuses an explicit `hooks_applied: false` response;
an omitted field is not treated as that explicit refusal, and hooks that
continue on failure never refuse startup. Failure behavior remains event-specific. The reviewed
TypeScript matcher contract ignores block failure policy for `Stop`,
`SubagentStop`, `TaskCompleted` and `TeammateIdle`, so it must not be presented
as a universal stop switch.

Permission callbacks answer requests the CLI sends to the host. CLI permission
modes and rules can authorize a tool before consulting that callback. An absent
Rust handler denies a host permission request; its absence does not mean that
every possible tool invocation is denied.

Hook names describe protocol surface, not guaranteed event delivery. The
reviewed Python hook union omits `SessionStart` and `SessionEnd`; Rust's enum is
broader. Installed CLI support and actual dispatch determine what runs.

## Shutdown and cleanup ownership

Use `disconnect().await` when the caller needs the cleanup outcome. It waits for the
owned driver, including bounded process shutdown/reaping and final storage
work. Typed errors retain process and timeout failures; transcript mirror
failures arrive as `mirror_error` system messages and do not fail the run. If a query is
abandoned early, explicitly close it to observe that outcome.
On operation failure, still await required cleanup and preserve both errors;
an early return through `?` otherwise leaves only the drop fallback.

Ordinary `Drop::drop` cannot await. Dropping the owner requests shutdown without
calling or awaiting `disconnect()`; it does not perform initialization again. The
child has a kill-on-drop fallback, but asynchronous cleanup needs a running
Tokio runtime. Dropping a value cannot report a failed final flush or promise
completed cleanup after runtime termination.

`Stop` and `SessionEnd` are CLI events, not host destructors. Explicit shutdown
cancels callbacks and stops normal protocol service before transport teardown;
pending or teardown-triggered hooks are therefore not guaranteed to run. Keep
required application cleanup in the application's own awaited shutdown path.

## Evidence and compatibility

This decision reviews Python `0.2.179` and published TypeScript `0.3.319`:

- [Python client positioning and lifecycle](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/client.py), including `receive_response()`, `disconnect()` and context management.
- [Python query internals](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/query.py) and [shared streaming initialization](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/client.py).
- [Published TypeScript source archive](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.3.319.tgz), inspected with verified integrity. Relevant `sdk.d.ts` contracts are `HookCallbackMatcher`, `Query`, `startup()` and `WarmQuery`; bundled runtime implements initialization, callback dispatch and cleanup.
- [Rust owner and query facades](../../src/client.rs) and [driver](../../src/driver.rs).

Public fixture regressions in [subprocess tests](../../tests/subprocess.rs)
cover process reuse, background continuation, callback cancellation, bounded
queues and owner-drop cleanup. [Runtime feature tests](../../tests/runtime_features.rs)
cover blocking-hook negotiation and host cancellation signals.
[Runtime session tests](../../tests/runtime_sessions.rs) cover first-turn
completion, mirror barriers and mirror failure reporting.

Source inspection and fixtures do not establish complete native model-session
conformance. Native hook execution, interrupt, background work and store-backed
resume still require appropriate CLI/version-specific conformance checks.

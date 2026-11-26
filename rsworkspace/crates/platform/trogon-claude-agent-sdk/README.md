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
            println!("{}", result.result.as_deref().unwrap_or_default());
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
`receive_response()` is Python's `receive_response()`: a `Stream` that yields
messages through the next `Message::Result` and then ends, leaving later
messages queued. `receive_messages()` streams on without stopping. `Query` and
`ClaudeSdkClient` are themselves `futures_util::Stream`s of
`Result<Message, Error>`, TypeScript's `for await`, ending where `next_message`
returns `None`.
`send_input(UserInput)` accepts structured content and every `SDKUserMessage`
field, written in declaration order with unknown keys after them.
`UserInput::from_json` writes a message object as given, as both SDKs write a
streamed message. `InputBlock` is a transparent JSON-object wrapper forwarded
verbatim, with a `text` convenience constructor; the CLI and the Messages API
validate block shapes.

Ordinary sends reject overlapping turns. Use the cloneable `ClientHandle` for
queued input or `stream_input`, and receive on the owner concurrently. Call
`finish_input()` when no more user messages will be sent. Reported queue counts
handle CLIs that coalesce queued sends into a turn.

`stream_input` follows TypeScript's `Query.streamInput()` by default: an abort
ends it quietly, a write after close fails with `Error::NotReadyForWriting`, and
stdin closes once the stream ends. When a callback needs the CLI and the call
wrote a prompt, stdin stays open until the run ends; both SDKs count the
prompts of each call, so a call that wrote nothing closes stdin at once even
while an earlier call's run is open. `try_stream_input` takes a fallible stream;
under TypeScript its error is returned and stdin stays open. Under
`ArgvStyle::Python` both log a failure, close stdin and return `Ok`, like
Python's `stream_input`.

`close()` on a `Query`, the client or any `ClientHandle` is TypeScript's
`Query.close()`: it takes `&self`, so it can be called while the stream is being
read, shuts the process down without waiting and is idempotent. Messages already
received stay readable before `next_message` returns `None`, and a control
request still waiting fails with `Error::QueryClosed` ("Query closed before
response received"); under `ArgvStyle::Python` it runs out its timeout instead,
as Python's `close()` leaves it. Call `disconnect().await` on the query or the
client to close and wait for shutdown and cleanup errors. A TypeScript
`for await` that breaks, the iterator's `return()` and `Symbol.asyncDispose`
are `disconnect().await`, which discards messages not read yet; `throw(error)`
is `disconnect().await` followed by returning the caller's own error, or
`abort()` to stop the process at once. Dropping the owner closes even when a
handle survives, but does not await cleanup. Cleanup needs a running Tokio runtime; the
child also has a `kill_on_drop` fallback.

## Pre-warming with startup

`startup(StartupOptions)` follows TypeScript `startup()`: it starts the CLI, completes the initialize handshake and
returns a `WarmQuery`. `WarmQuery::query(prompt)` writes the prompt to that process and returns a `Query`;
`close()` discards the process without a prompt, and `dispose()` closes it and waits for the cleanup. Python
0.2.181 has no pre-warming. `query` takes `self`, so a second call cannot compile where TypeScript throws.

`initialize_timeout` of `None` waits 60000 ms and does not count the time in which Claude Code reports a
SessionStart or Setup hook running, for at most 10 minutes; a value you pass counts throughout. A timeout is
`Error::InitializeTimeout` with the upstream text, the `initialize_timeout` error class, the last startup mark
(or `none`) and its age. The process gets `CLAUDE_CODE_SDK_READS_STARTUP_MARKS=1` unless the environment has that
key in any letter case. An error result exit carries `ResultError::startup_failure_reason`.

## Spare processes with prewarm

`prewarm(StartupOptions)` follows TypeScript `prewarm()`: it starts the CLI with `--await-claim`, completes the
initialize handshake and returns a `SpareProcess` that is not tied to a session yet.
`SpareProcess::claim(prompt, ClaimOptions)` writes one `claim_session` request (working directory, permission mode,
environment, extra directories, system prompt addition, title, agents, accepted workspace trust and the SDK MCP server
names), applies the model, thinking budget and settings around it and then sends the prompt, returning a `Query`.
`claimed()` yields the `ClaimInfo`, `exited()` resolves when the process ends, and `close()` or `dispose()`
discard the spare. A claim fails with `ClaimError` carrying the upstream text: `settings_not_applied` closes the
process and sends no prompt, `option_not_applied` sends the prompt and says which options were skipped. A spare
started without a directory parks under `spares/spare-XXXXXX` (mode 0700) in the config home until it is claimed.
Settings that register hooks cannot be applied by a claim and are refused before anything is written.
The claim answer's `initialize` replaces the initialization result as `sdk.mjs` merges it: the applied flags it
omits or sends as `null` keep their first values, and a first result that was missing or `null`, or an `initialize`
of `null`, fails the initialization result with `Error::TypeError` while `claimed()` still succeeds.

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

Under the default TypeScript dialect `connect().await` and `query()` return as soon as the initialize
request is queued, the way `sdk.mjs` hands back its `Query`; anything sent next is written right behind
the initialize bytes. Under `ArgvStyle::Python`, `connect()` and `query()` wait for the initialize
response, as `claude_agent_sdk` does. `ClientHandle::initialization_result().await` returns the
`ConnectionInfo` of the first initialize response, including support flags and additional metadata, and
waits for it when it has not arrived. A failed initialize rejects it with `Error::InitializationFailed`,
which prints the underlying error; the run ends with that error too when a blocking hook or managed
settings made the prompt wait for the confirmation, and otherwise it carries on, as `sdk.mjs` does. It
sends no protocol messages; `ConnectionInfo::as_json()` retains the original
response, including omitted or explicitly null fields, or `None` when the reply
has no `response` field, where TypeScript `initializationResult()` resolves to
`undefined`. Like both upstream SDKs,
the response is not validated: a known field of an unexpected type reads as
absent, and under `ArgvStyle::Python` a response that is not an object reads as
`{}`. The initialize checks follow `sdk.mjs` in its order: blocking hooks need
`hook_on_failure_supported`, then `hooks_applied` must not be `false`, then sent
managed settings need `managed_settings_applied`; plugins that were not
confirmed only log a warning. Non-empty `supported_dialog_kinds` without a
`user_dialog_handler` is refused before the CLI starts.
The handle provides every `Query` control of the pinned TypeScript SDK:
interrupt receipts, model, permission and thinking-token changes, flag and
settings updates, rewind, MCP status, permission override, resource reads and
reconnect/toggle/replacement, context and usage reports, file reads, plugin,
skill and output-style reloads, read-state seeding, task stop and backgrounding,
and `reinitialize`. Each request matches `sdk.mjs` byte for byte by default and
`claude_agent_sdk` under `ArgvStyle::Python`, with optional fields omitted when
unset. Replies come back as `ControlReply<T>`: `as_json()` keeps the value the
CLI sent, or `None` when the reply has no `response` field, where TypeScript
returns `undefined`, and `decode()` reads it as the generated upstream type.
Under `ArgvStyle::Python` an absent, null or non-object `response` reads as
`{}`, as `claude_agent_sdk` does.
`read_mcp_resource` follows TypeScript `readMcpResource`: Claude Code reads the
`ui://` resource, and the reply's `response` field is returned as sent, without
a shape check, or `None` when the reply has none, where TypeScript returns
`undefined`. Each method returns what its TypeScript counterpart returns
(`read_file` is `None` on any failure, `background_tasks` is `true` when the CLI
omits `backgrounded`), and a rejected request keeps a well-formed TypeScript
`error_code`.
`supported_commands()` follows the latest `commands_changed` system message,
after reading the initialization result as `sdk.mjs` does. When initialize
answered without a `response` or with `null`, `supported_commands`,
`supported_models`, `supported_agents` and `account_info` fail with
`Error::TypeError`, the engine text `ClaudeAgentOptions.js_runtime` selects.
Legacy interrupt replies remain distinct from reported empty queues. Newer
controls depend on installed CLI support.

## Callbacks and MCP

`ToolPermissionHandler` receives every TypeScript `canUseTool` option:
request and tool identities, suggestions, MCP provenance, the decision reason
with its type and code, the matched ask rule, the computer folder, plugin and
server prompt, plus a cancellation signal. A field of the wrong type stays in
`additional_fields`. Decisions carry updated input, permission updates,
interrupt and a decision classification; `AnsweredOutOfBand` writes nothing,
and `Raw` writes any other object as returned (Python rejects it as a `dict`).
Answers match `sdk.mjs` byte for byte by default and `claude_agent_sdk` under
`ArgvStyle::Python`, including the answer without a handler, a callback the CLI
cancels (TypeScript still writes its answer, Python writes nothing) and a
request id redelivered while in flight (skipped by default, run twice with
`DuplicateControlRequests::RunBoth`). An opted-in `limits.callback_timeout`
denies.
The CLI can authorize tools through its own permission mode or rules before
consulting the host.
Suggestions keep unknown destinations, behaviors and update types verbatim, so
they round-trip into an allow decision. A permission request that cannot be
decoded gets an error response and the session continues.

`HookRegistration` and `HookOutput` support hook callbacks.
`HookRequest::typed_input` decodes the input into the generated upstream
`HookInput`, and `HookOutput::Json` sends any upstream `HookJSONOutput` as
given. Blocking failure semantics require advertised CLI support and
registered hooks; startup refuses incompatible responders. [The hooks example](examples/hooks.rs) blocks shell
commands. Elicitation and user-dialog callbacks also receive the request id
and a cancellation signal. As in `sdk.mjs`, every elicitation mode and dialog
kind reaches the handler and `supported_dialog_kinds` is only advertised to the
CLI. Without a handler an elicitation is declined and a dialog is left for
another client to answer.

Under the default TypeScript dialect, `ClientHandle::set_ui_host` takes a
`UiHost` whose methods answer the CLI's six `ui_*` requests; a missing method
falls through to the unsupported subtype error. Under `ArgvStyle::Python` every
`ui_*` request gets that error.

`ClientHandle::enable_remote_control` takes `RemoteControlOptions` and, under
the default TypeScript dialect, answers `remote_control_work_secret` for the
session it attached, matched as `sdk.mjs` does. It returns the `response` as
sent: `None` is `undefined` and `Some(null)` is `null`. Enabling with either one
fails with `Error::Protocol` while it is still the latest call, as `sdk.mjs`
throws reading `bridge_session_id`. `sdk.mjs` can call `enableRemoteControl`
before `initialize` resolves, which writes it right behind the initialize
request. The TypeScript dialect's `connect` returns at that point, so a call
made on the returned client lands at the same place on the wire. Under
`ArgvStyle::Python` a work secret request gets the unsupported subtype error.

The run ends as `hasBidirectionalNeeds` decides. When nothing needs the CLI to keep
asking (no embedded MCP server, hook or permission handler, and under TypeScript
no elicitation or dialog handler, auth callback or work secret answerer), a streamed
input closes stdin as soon as it is written. Otherwise stdin closes when the run
ends, and only the frames decide that, per dialect (`src/run_end.rs`). Under
TypeScript a result ends the run when the session reported no state or is idle, and
otherwise `idle` does, or the ceiling (`CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`, read
and parsed as `limits.run_end_timeout` selects, 600000 ms by default and
disabled by 0) when no `idle` follows; reported tasks
and queued turns play no part. `query()` with a prompt is `sdk.mjs`'s single user turn,
which closes stdin the moment its run ends. With a session store, a result reaches that machine only after its transcript flush, as in both
readers; the ceiling and a finished input do not wait for a flush. Under `ArgvStyle::Python` the prompt waits
the same way, but a tracked `local_agent` or `local_workflow` task still in flight keeps
the run open at a result, at `idle` and at the ceiling. `query()` and the client keep
yielding what the CLI sends after the result until the CLI exits; only `query_turn()`
stops at the first result. A CLI that never sends a result still gets its stdin closed
after the last prompt when nothing needs it.

[The feature-connect example](examples/feature_connect.rs) checks blocking-hook
registration and MCP connection without a prompt. It refuses startup when the
installed CLI does not advertise blocking-hook support. Verification with CLI
`2.1.312` exercised that refusal; CLI `2.1.319` accepted registration
and connected MCP. Pass an executable path as the example's first argument to
check a separate CLI installation.

Build a `LocalMcpServer` with named `LocalMcpServerOptions` fields. Define each
schema-validated tool through `SdkTool::from(SdkToolOptions { ... })`, naming its
metadata, input schema and async handler. Construct its `McpToolName` and
`McpToolInputSchema` first. `McpToolName` stores any text, and `tools/list`
lists every name. In the TypeScript dialect, `LocalMcpServer::from` logs each
line `createSdkMcpServer` prints with `console.warn` for a name the MCP tool
naming standard discourages, one `tracing` warning per line and in the same
words. That covers names outside one to 128 letters, digits, underscores,
hyphens or periods, and also valid names that start or end with a hyphen or a
period. The Python dialect logs nothing, as `create_sdk_mcp_server` builds a
lowlevel MCP server that does not check names.
`McpToolInputSchema::new` keeps any schema verbatim, as both upstream SDKs do;
a call to a tool whose schema does not compile returns that error as the tool
result. `McpToolInputSchema::validated` opts in to requiring a compiling object
schema without external references. `SdkTool::from` then assembles its inputs
without returning an error.

A handler reads the validated arguments as a `JsonObject` through
`McpToolRequest`, or as its own type through `TypedMcpToolHandler`, like the
typed handler of the TypeScript `tool()` and the Python `SdkMcpTool[T]`. The
typed handler reads the arguments with `serde` after the server validates them
against the input schema. Arguments that pass the schema but do not fit the type
never reach the handler; the call answers with the dialect's invalid arguments
result, `MCP error -32602: Input validation error: Invalid arguments for tool
<name>: <message>` under TypeScript and `Input validation error: <message>`
under Python, carrying the `serde` message. A handler may fail with
`McpToolArgumentsError` itself to report invalid arguments the same way:

```rust,no_run
use std::sync::Arc;

use serde::Deserialize;
use trogon_claude_agent_sdk::{
    McpToolInputSchema, McpToolName, McpToolOutput, SdkTool, SdkToolOptions, TypedMcpToolHandler, TypedMcpToolRequest,
};

#[derive(Deserialize)]
struct Repeat {
    text: String,
    times: usize,
}

fn repeat_tool(input_schema: McpToolInputSchema) -> SdkTool {
    SdkTool::from(SdkToolOptions {
        name: McpToolName::from("repeat"),
        description: "Repeat the supplied text".into(),
        input_schema,
        handler: Arc::new(TypedMcpToolHandler::new(|request: TypedMcpToolRequest<Repeat>| async move {
            let Repeat { text, times } = request.arguments;
            Ok(McpToolOutput::text(text.repeat(times)))
        })),
        annotations: None,
        search_hint: None,
        always_load: false,
    })
}
```

`LocalMcpServerOptions.tools` requires an immutable `SdkTools` collection.
Use `tool.into()` for one tool, `SdkTools::default()` for none, or
`SdkTools::new([first, second])?` to reject duplicate names when assembling
several tools, as the TypeScript SDK does. Exact names must be unique;
case-distinct names remain separate. `SdkTools::with_duplicates` keeps repeated
names, as the Python SDK does. `LocalMcpServerOptions::new(name)` starts from no
tools; `version`, `instructions` and `always_load` mirror the upstream server
options. The Python dialect ignores `instructions` and `always_load`, because
`create_sdk_mcp_server` takes only a name, a version and tools. `LocalMcpServer::from` assembles these inputs infallibly.
Each client opens its own session. The built-in server supports initialize,
initialized notification, cancellation, ping and tool listing/calls.
`McpServerDialect::TypeScript`, the default, answers like `createSdkMcpServer`:
it drops messages it cannot read, reports parameter errors as `-32603` and
lists each schema with its draft-07 `$schema`. It validates tool arguments,
handler results and the JSON-RPC envelope the way the bundled zod and MCP SDK
do, with their issue paths, codes, messages and order. The zod schema is
rebuilt from the listed JSON Schema, so what JSON Schema cannot tell apart
follows the mapping zod's own `toJSONSchema` makes most likely, until the tool
author chooses otherwise with `SdkTool::with_validation`. A
`ValidationChoices` (built from `ValidationChoicesOptions`, with typed errors)
keeps its choices beside the schema, keyed by the JSON Pointer of the node they
apply to, so `tools/list` stays exactly what `createSdkMcpServer` sends. Each
choice has a recorded answer from the real zod in
`tests/fixtures/sdk_mcp/typescript_choices.json`:
`UnionChoice` picks a plain union (every option's issues in one
`invalid_union`) or a discriminated union (the option the discriminator names,
or one `invalid_union` at the discriminator); `NullabilityChoice` picks
`nullable` (the inner issues) or a union with `null` (an `invalid_union`);
`LengthChoice` picks `length(n)` (one `exact` issue) or `min(n).max(n)`;
`SchemaChoice::StringCheckOrder` lists the minimum, maximum and format checks
of a string in the order they were declared, which is the order of its issues;
`NumberFormatChoice` picks `int32`/`uint32` (the format names the integer type)
or `number().int().min(..).max(..)` (`safeint`); `StringFormatChoice` picks
`iso.time()` (format `time`) or a plain `regex` pattern; and
`SchemaChoice::RegexFlags` gives the flags the listed `pattern` loses.
`ObjectChoice`, recorded in `tests/fixtures/sdk_mcp/typescript_zod.json`, picks
the bundled object a raw shape is wrapped in (an absent required key whose
schema takes `undefined`, such as a zod 4.1.12 `catch`, reports `nonoptional`)
or the author's own `z.object` (the key keeps the value its schema gives).
`SchemaCallbackOptions` opts a schema node in to a `refine` and a `transform`
callback, which run where zod runs them: the refinement after the node's checks
unless one aborted them, as a `custom` issue with its path and message, and the
transformation only when nothing failed, its result being what the handler
receives. The embedded-server tests are
`typescript_dialect_validates_each_exposed_choice_like_zod` and the unit tests
in `sdk_mcp_choices_tests.rs`. Patterns run through an ECMAScript
regular expression engine (lookaround, backreferences, named groups and the `u`
and `v` flags), and a pattern that does not compile fails when the tool is
registered. A handler result is passed to the client as the JSON value it
returned once it passes `CallToolResult`.
Schemas that use keywords zod cannot express (`allOf`, `oneOf`, `$ref` and the
like) are also checked by the JSON Schema validator. `McpServerDialect::Python`
answers like `create_sdk_mcp_server`: it reports unreadable messages as
`McpServerError::Protocol`, refuses requests before `initialize` and answers a
cancelled call with `-32800`. Both dialects validate `clientInfo.icons` and
`capabilities.elicitation` (`form` and `url`) like the pinned schemas: zod issues
for TypeScript (unknown icon keys are stripped, `elicitation.url` must be an
object or array), the generic `-32602` for Python (`null` is accepted for every
optional field). The server keeps and echoes neither. In the Python dialect a request id round-trips
exactly as `json.loads` and `json.dumps` keep it: integers of any size and `-0`
are echoed digit for digit, and floats, booleans, `null` and nested values are
not request ids, so a found server acknowledges them without one. A custom
`Transport` that reads lines can keep the ids by scanning each line with
`McpWireScan` and returning `McpWireScan::into_ids` from
`Transport::take_wire_ids`. The TypeScript dialect keeps `JSON.parse` numbers,
so an id beyond `Number.MAX_SAFE_INTEGER` is echoed as the double it rounds to.
Custom
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
verbatim as an `McpConfig`. Debug output redacts it, and the `extra` keys of
every server config. An in-process server replaces an external config of the
same name.

`mcp_message` control requests are routed the way the selected style's SDK
routes them. TypeScript fails the request for an unknown server or a message
that is not an object, waits for the server's response to a request with an
id, and acknowledges anything else with an id 0 result. The error for a
message that is not an object is the `TypeError` the JavaScript engine raises,
so `ClaudeAgentOptions.js_runtime` picks its words: `JsRuntime::Node` (the
default) names the value as V8 does, and `JsRuntime::Bun` gives the fixed
JavaScriptCore text. TypeScript keys the
waiting request by the JavaScript text `${server_name}:${id}`, so ids `1` and
`"1"` collide: the later request replaces the earlier waiter, and the earlier
control request is never answered. It has no timeout on that wait. A message a
server sends with no waiter for its id (a notification, a request to the client,
a repeated or unmatched response; see `SdkMcpSession::take_outbound`) goes to
the CLI as an `mcp_message` control request under a random UUID, except a late
`sdk-manifest-capture:` reply, which is dropped. The forwarded frame is written
as `{type, request_id, request}`, and is held until the `initialize` request has
been written, then flushed in order. A `notifications/tools/list_changed`
without an `id` forwarded before that point marks its server, and the
`initialize` request then keeps only the `initializeResult` of that server's
manifest. Closing writes nothing for the requests still waiting. Python fails the
request when the server name or message is missing, and answers an unknown
server or an undeliverable message with a JSON-RPC error under the message's
id.

The Python dialect reads every message with the `mcp` package's JSON-RPC union
(request, notification, response, error response), including pydantic's lax
reading of an error code and its smart-union choice between models. A message
no model accepts is reported with the text of the `ValidationError` pydantic
raises: the same header, locations, messages, truncated `input_value` repr and
documentation link.

Before the first `initialize` request, TypeScript style captures a manifest for
each in-process server (`captureSdkMcpServerManifests`), so the CLI need not
handshake them over the control channel. Each server gets an `initialize`
request with id `sdk-manifest-capture:initialize` (protocol version
`2025-11-25`, no capabilities, client `claude-code` `0.3.323`), then a
`notifications/initialized`, then, when its capabilities include `tools`, a
`tools/list` with id `sdk-manifest-capture:tools-list`. The manifest is
`{initializeResult, toolsListResult}`, and `toolsListResult` is left out when
the result has a `nextCursor`. A server that answers an error, throws or does
not answer is left out and the CLI handshakes it. All servers run side by side
under one 250 ms deadline, and a reply after it is dropped. The manifests go in
`sdkMcpServerManifests` of the `initialize` request. Upstream turns the capture
off with the `CLAUDE_AGENT_SDK_DISABLE_MCP_MANIFESTS` environment variable and
has no query option for it; `ClaudeAgentOptions::capture_sdk_mcp_manifests` is
this crate's own switch and `Some(false)` disables it. Python style has no
capture.

The Python dialect validates tool arguments as the bridge does
(`claude_agent_sdk/__init__.py`, `_build_input_schema` at lines 436-456 and
`jsonschema.validate` near line 630): `jsonschema` 4.26 with the draft
`validator_for` picks from `$schema`, `referencing` 0.37 reference resolution and
the `best_match` error choice, reported as `Input validation error: <message>` or,
for any other exception, its text. As in `jsonschema.validate`, the schema is
checked first with `check_schema`: the first error the draft's metaschema yields,
checked with the draft's `FORMAT_CHECKER` (only `regex` is registered in the
pinned environment), is reported as the `SchemaError` text. The arguments are
then checked with `format` as an annotation only, because the bridge passes no
`FormatChecker`. Patterns follow a port of CPython's `re`. Python's recursion
limit is counted frame by frame as CPython 3.12 counts it, for a bridge started
with `asyncio.run` from module level (`jsonschema.validate` at frame 17 under
the default limit of 1000) that already handled a call. Neither the Python SDK
nor `mcp` raises that limit, and the dialect has no setting that does. A schema or arguments
nested past that boundary give the same `maximum recursion depth exceeded` text,
and a `SchemaError` too deep to print raises inside the bridge's handler, which
the `mcp` server answers with a JSON-RPC error of code 0. A pattern `re` still
holds compiled costs no compile frames, as in `re._compile`: its first-in
first-out cache of 256 patterns and its least-recently-used cache of 512 are
replayed across every call of every server in the process, so a pattern one call
compiled is cached for the next until 512 newer patterns evict it. Printing an
error uses those caches too: `pprint` splits a long string with
`re.findall(r'\S*\s*', line)` and `textwrap.dedent` substitutes the error's
margin with `re.sub`, and each compiles its pattern, six or seven frames deeper
than a cached lookup, the first time the process needs it. A `SchemaError`
printing a long string under 324 lists passes the limit while compiling the
word pattern, and prints once an earlier error compiled it. The caches start as
the pinned SDK's process holds them at its first tool call on macOS, filled by
the imports of `claude_agent_sdk` and its dependencies and by the version check
while connecting. A Python program that imports other modules, or runs on
another platform, starts with other patterns cached, which moves a boundary only
for a pattern nested to within a level of it. The repr of a value in an error
message overflows near 10,000 levels on C recursion, which the schema's nesting
does not move: one level sooner for a call of the `repr` builtin, as in
`type_draft3`, and four sooner for a repr joined from a generator expression,
as in `extras_msg` and the `oneOf` message. Comparing values nested near
10,000 levels overflows on C recursion too: the `list.sort` of `_utils.uniq`,
which `check_schema` runs for the `enum` of drafts 3 and 4, raises
`maximum recursion depth exceeded in comparison`, and the `all` calls of
`_utils.equal`, which `enum` and `const` call, hold checks that a repr beneath
them counts. The first call of a process overflows a few levels sooner than the
next: CPython runs an instruction generic once before specializing it, and the
generic `sorted` and `next` calls and the loops over generators in
`check_schema`, `iter_errors` and `descend` hold C recursion checks the
specialized ones do not. Which of them the process already ran is kept with its
`re` caches. The validator converts, compares, prints and drops values with
loops over explicit stacks, so a value nested that deep does not overflow the
native stack. In Python the boundary moves by a level when the caller sits a few frames deeper, as under another
event loop setup or a test runner. The TypeScript dialect reads messages from
the CLI at any nesting. The Python dialect fails with `Error::Recursion` where
the Python SDK's reader raises `RecursionError` from `json.loads`: at the first
object or array past 9,991 levels, counting the message itself, once the text
before it reads as JSON. An unterminated last line fails too. Below those
limits, a message is parsed, decoded and routed without recursing over its
values, and the message types, `MessageParseError` and `ResultError` drop the
values they own with a loop, so nesting never overflows the native stack.
`Message::Unknown` and `ContentBlock` hold their JSON as `DeepJsonObject` and
`DeepJsonValue`, which clone and drop it with a loop, so a destructured variant
or a block moved out of its message drops the same way. The exceptions are a
payload taken out with `into_object` or `into_value`, which drops as
`serde_json` does, and the conversions a caller runs, such as `Clone` of the
message structs, `Debug`, serialization and `to_sdk_message`. The
TypeScript dialect runs no JSON Schema validator: it emulates the zod object
the package builds.

Where `jsonschema` and `referencing` iterate a `set` of strings, the Python
dialect iterates it in CPython 3.12's order: the slots of its hash table, from
`str` hashes under the process's hash secret. That order decides which
`additionalProperties` extra fails first when one raises, such as an
unresolvable `$ref`, and which of several resources sharing an `$id` or an
anchor the crawl keeps. The secret follows `PYTHONHASHSEED` as Python reads it
at startup: unset, empty or `random` gives a secret of its own to each process,
and a seed from 0 to 4294967295 gives the secret Python derives from it, so a
fixed seed reproduces a recorded order. A value Python refuses to start with is
taken as random. The keyword sets of `referencing.jsonschema` are iterated as
the `.pyc` CPython compiles them, which marshals each constant `frozenset` in
sorted order; a module run without its cached `.pyc` builds them in source order
instead. The order of `rpds` hash sets is not reproduced: its only set iterated
while validating holds at most one URI.

Like `jsonschema.validate`, the Python dialect retrieves a `$ref` that is outside
the schema and the bundled metaschemas, as `_REMOTE_WARNING_REGISTRY` does. It
fetches through a `RemoteSchemaRetriever`, set with
`LocalMcpServer::with_remote_schema_retriever`. The default `UrlopenRetriever`
ports CPython 3.12 `urllib.request.urlopen` for `http`, `https`, `ftp`, `data`
and `file` URLs, with its redirects, exception classes and texts, and with the
proxies `getproxies_environment` reads from the `<scheme>_proxy` and `no_proxy`
variables or, when no variable names one, from the macOS system configuration as
`_scproxy` reads it and from the Windows registry as `getproxies_registry` and
`proxy_bypass_registry` read it. Over `https`, a server that breaks the protocol
before encryption starts raises the `ssl.SSLError` text CPython 3.12 reports
over OpenSSL 3.5.
A failed retrieval makes the reference `Unresolvable`. Each response that opens logs the
`DeprecationWarning` of `_warn_for_remote_retrieve` as a `WARN` event with the
target `trogon_claude_agent_sdk::python_warnings`. The event carries `category`,
`message`, `filename`, `lineno`, `source` and `uri`, and reaches the caller's
`tracing` subscriber even though validation runs on its own thread. Python's
default filters hide that warning, so it shows only where a subscriber keeps the
target. A retrieved document loads as `json.load` loads it, with `NaN`,
`Infinity`, integers beyond 64 bits and lone surrogates kept. A `$ref` URI
holding a lone surrogate reaches the retriever through
`RemoteSchemaRetriever::open_python`, which by default raises the
`UnicodeEncodeError` of encoding it as UTF-8 and which `UrlopenRetriever` opens
as `urlopen` opens it.

`set_mcp_servers` is not serialized, as in TypeScript. It swaps in-process
servers before the request is sent and keeps the session of a server that stays
registered. Remove a server and re-add it to replace its registry instance. Its
reply is a `ControlReply<McpSetServersResult>` kept as the CLI sent it, like
the other controls.

## Configuration and limits

The interactive client follows Python's `ClaudeAgentOptions` and `ClaudeSDKClient`
terminology, with Rust acronym casing. Its core optional fields default to `None`:

| Rust field | Python field | TypeScript option |
| --- | --- | --- |
| `cli_path: Option<PathBuf>` | [`cli_path`](https://github.com/anthropics/claude-agent-sdk-python/blob/22b8eb9c19adfa78759b79e22bd664a57843bb06/src/claude_agent_sdk/types.py#L2469) | `pathToClaudeCodeExecutable` |
| `cwd: Option<PathBuf>` | [`cwd`](https://github.com/anthropics/claude-agent-sdk-python/blob/22b8eb9c19adfa78759b79e22bd664a57843bb06/src/claude_agent_sdk/types.py#L2466) | `cwd` |
| `model: Option<ModelName>` | [`model`](https://github.com/anthropics/claude-agent-sdk-python/blob/22b8eb9c19adfa78759b79e22bd664a57843bb06/src/claude_agent_sdk/types.py#L2440) | `model` |
| `permission_mode: Option<PermissionMode>` | [`permission_mode`](https://github.com/anthropics/claude-agent-sdk-python/blob/22b8eb9c19adfa78759b79e22bd664a57843bb06/src/claude_agent_sdk/types.py#L2395) | `permissionMode` |
| `setting_sources: Option<Vec<SettingSource>>` | [`setting_sources`](https://github.com/anthropics/claude-agent-sdk-python/blob/22b8eb9c19adfa78759b79e22bd664a57843bb06/src/claude_agent_sdk/types.py#L2670) | `settingSources` |

`PermissionMode` mirrors Python's
[`PermissionMode`](https://github.com/anthropics/claude-agent-sdk-python/blob/22b8eb9c19adfa78759b79e22bd664a57843bb06/src/claude_agent_sdk/types.py#L25)
and the exported type in
[TypeScript `0.3.323`](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.3.323.tgz).
Serde uses their literal names: `default`, `acceptEdits`, `plan`,
`bypassPermissions`, `dontAsk`, and `auto`. `None` leaves the mode unset;
`Some(PermissionMode::Default)` explicitly selects `default`. The runtime omits
the flag for `None` and sends the explicit literal for `Some`.

`SettingSource` uses the exact serde literals `user`, `project`, and `local`.
Python's pinned `setting_sources` docs and TypeScript `0.3.323`'s `Options.settingSources`
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
- `EnvironmentMode::TypeScript` (default) replaces the inherited environment with
  `options.env` when it has entries and inherits it otherwise, as TypeScript
  `0.3.323` does. `Replace` and `Inherit` pick either branch explicitly (`Replace`
  with an empty `options.env` is TypeScript's explicit `env: {}`). `Overlay`
  (Python): inherited environment, then `options.env`. `CLAUDECODE` is removed in
  `Overlay` unless `ClaudeCodeMarker::Keep` is chosen, and kept otherwise unless
  `ClaudeCodeMarker::Remove` is chosen.
  `CLAUDE_CODE_ENTRYPOINT` defaults to `sdk-trogon-rs` and is configurable.
  `CLAUDE_CODE_SDK_READS_SESSION_STATE=1` is added unless a case-insensitive match
  exists; the inherited check covers only the variable names the SDK captures.
- `McpDiscovery` defaults to `CliDefault`; `--strict-mcp-config` is emitted only for
  the explicit `Disabled` opt-in. `--permission-prompt-tool stdio` is sent only
  when a tool permission handler exists. `HookRegistration::on_failure` is an
  `Option` and the wire field is omitted when unset.
- `ClaudeSdkClient::connect` discovers the CLI in Python's order when `cli_path` is
  omitted, starting with `<root>/_bundled/claude[.exe]` when `bundled_cli_root`
  names Python's package directory (off by default, because this crate ships no
  binary and TypeScript has no bundled lookup); a miss is `CliNotFoundError`
  with Python's hint text. `connect_with_adapters` never discovers, matching
  Python's custom transport and TypeScript's `spawnClaudeCodeProcess`: its
  spawner receives `claude` when `cli_path` is omitted.
- `Transport` is Python's transport interface: `connect`, `write`, `read_message`
  (one object per call, `None` at the end of the stream, in place of Python's
  `read_messages` iterator), `close`, `is_ready` and `end_input`, plus a provided
  `kill` that `abort()` calls before `close` (TypeScript's abort signal) and a
  provided `accepts_writes`, `is_ready` unless the transport's `write` waits for
  its own connection, as the SSE transport's does. It is a
  generic bound, not a trait object. `ClaudeSdkClient::connect_with_transport`,
  `connect_with_transport_and_adapters`, `query_with_transport` and
  `query_turn_with_transport` inject one; the subprocess implementation is the
  default and stays crate-private. A custom transport gets no CLI discovery, no
  argument list and no store-backed resume materialization, while the session
  store mirror still runs, as in Python. `read_message` must be cancel safe, and
  an `Err` from any method ends the run as `Error::Transport` surfaces it.
- `extract_from_bunfs` ports the TypeScript `extract` entrypoint. A path with
  neither `$bunfs` nor `~BUN` comes back unchanged; any other is read, hashed
  (first 16 hex digits of SHA-256) and copied to
  `{tmpdir}/claude-agent-sdk-{uid}/claude-agent-sdk-{hash}/<name>` (root
  `claude-agent-sdk` on Windows, tmpdir from `CLAUDE_CODE_TMPDIR`, `/tmp` on macOS
  or the OS temp dir). An existing copy is reused only when it is a plain,
  singly linked file with the same bytes. Failures are `tracing` warnings with the
  upstream text and the input path is returned. `extract_from_bunfs_with` takes an
  `ExtractHost` for tests. Warning texts name errnos from a fixed table, and
  hard link counts and file ids are only read on Unix.
- `spawn_claude_code_process` is TypeScript's `spawnClaudeCodeProcess`: the SDK
  builds the command and hands `SpawnOptions` (command, arguments, working
  directory, the whole child environment, a forwarded abort signal and, on Unix,
  the uid from `user` or `run_as_uid`) to the callback, which returns a tokio
  `Child` with piped stdin and stdout, so a container or remote shell can host the
  CLI. The forwarded signal is cancelled only when the client closes after its
  abort controller was cancelled, past the close grace. As in TypeScript, a
  custom spawn gets no default debug file. The SDK's own spawn passes the child
  only its stdio under the Python dialect on every platform, as
  `Popen(close_fds=True)` does, and under the TypeScript dialect on Apple
  platforms, as libuv's `posix_spawn` does; a custom spawn decides which
  descriptors its child keeps. `on_process_spawned` is TypeScript's
  undeclared `onProcessSpawned`: after the SDK's own spawn it receives the pid
  and `spawn_sync_ms`, and a panic in it is logged and ignored.
- `find_bundled_cli` ports Python's `<package>/_bundled/claude[.exe]` check, which
  `connect` runs only through `bundled_cli_root`.
  `bundled_cli_derives_client_app` and `meets_derived_client_app_minimum` port
  Python's version gate for deriving an app label (the pinned CLI version and the
  `claude.version` record beside the bundled binary must both be `999.0.0` or
  later); `connect` asks them only through the Python dialect
  `client_app_derivation` option and otherwise follows TypeScript.
  `find_native_cli` ports TypeScript's platform package selection
  (`@anthropic-ai/claude-agent-sdk-<platform>-<arch>`, musl before glibc on musl
  Linux, `-android` packages, `.exe` on win32) over a `ResolveNodeModule`
  adapter; `NodeModuleLookup` is the default and searches Node's `node_modules`,
  `NODE_PATH`, home and prefix directories. `NativePackageTarget::host` prefers
  musl on Linux unless the process reports a glibc runtime version
  (`gnu_get_libc_version`), as the Node process report does.
- Failures follow upstream. A missing executable is `Error::CliNotFoundAt`, a missing
  `cwd` is `Error::WorkingDirectoryMissing`, any other spawn failure is
  `Error::Spawn` ("Failed to start Claude Code: ..."). `Error::Process` displays
  Python's `ProcessError` text, exposes `exit_code()` (negated signal number on
  Unix) and `typescript_message()` for TypeScript's text. When the CLI exits
  unsuccessfully after an `is_error` result, the error is `Error::Result`
  (`ResultError`): Python's text always, TypeScript's text when it is non-empty.
  `abort()` stops the process and ends the run with `Error::Aborted`
  (`AbortError`, "Claude Code process aborted by user"). It cancels
  `abort_controller`, TypeScript's `abortController`: a caller's token cancelled
  from anywhere does the same, a token already cancelled fails `connect` with
  that error, and `close()` leaves it untouched. Closing follows `limits.shutdown`: TypeScript's ladder by
  default (stdin close, 2 seconds, SIGTERM, SIGKILL 5 seconds later if the
  process is still running, without waiting), Python's 5/5/5 second ladder, or `Shutdown::Strict`, which reports
  `Error::ShutdownTimeout` and `Error::StderrDrainTimeout`.
- Writes fail with distinct errors in upstream's order: `Error::NotReadyForWriting`
  after stdin is closed (and, in the TypeScript style, after the process exited,
  where the exit event clears `ready`), `Error::WriteToTerminatedProcess` after an
  exit in the Python style (with the exit code), `Error::WriteFailed` for an I/O
  failure and `Error::Aborted` after `abort()`.
- Non-JSON stdout: `ArgvStyle::TypeScript` reads lines as Node `readline` does
  (split at `\n`, `\r\n`, a lone `\r`, U+2028 and U+2029, decoded with
  replacement characters), skips a blank line, parses the rest with `JSON.parse`
  rules (object keys in `Object.keys` order, every number a double) and skips
  every undecodable line. A `null` line is `Error::NullMessage` with V8's
  `TypeError` text. Python style decodes stdout as strict UTF-8
  (`Error::UnicodeDecode` with the `UnicodeDecodeError` text, whose position
  Python counts within the chunk it read), strips the whitespace `str.strip`
  takes, skips lines that do not start with `{` and fails on a corrupt line with
  `Error::JsonDecode` (the line and the original error), except that a corrupt
  unterminated tail at end of file is dropped. An integer past 4,300 digits is
  `Error::IntDigits` with the `ValueError` text, on any line. Values the public
  types cannot hold are handled as follows: Python's `NaN`, `Infinity`,
  out-of-range floats such as `1e400` and lone surrogate escapes are
  `Error::JsonDecode` on a terminated line and dropped in a tail; an integer
  beyond 64 bits arrives as the nearest double, except one too long for a double,
  which is `Error::JsonDecode` (or dropped in a tail); TypeScript lines holding
  `Infinity` or a lone surrogate, and lines that parse to something other than
  an object, are skipped. With `MessageDecoding::Python`, a
  known message missing a key Python reads is `Error::MessageParse`, carrying the
  raw object.
- `ArgvStyle::TypeScript` appends `--await-initialize` through extra arguments
  when `connect` resumes or continues without `cli_path` and without `sdk-url`,
  as `sdk.mjs` does for its resolved executable. Python never sends it.
- `user` runs the CLI as the named account (`Popen(user=...)`: the name is
  resolved with libc `getpwnam_r`, so NSS sources apply, an unknown name fails
  with `getpwnam(): name not found: '<name>'`, and only the real and effective uid change through `setreuid`, as in CPython, Unix
  only); `run_as_uid` is the numeric form.
- `extra_args` behave like the upstream dictionaries: a repeated flag keeps its first position and its last value,
  TypeScript lists integer-like flags first in ascending order, and TypeScript writes `--flag=value` for a value
  longer than one character that starts with `-` (a lone `-` is a separate token) where Python writes it for any
  value that starts with `-`. An empty `agent` or `resume` is skipped, as upstream skips falsy values.
- `ToolSet::ClaudeCodePreset` is the `{type: "preset", preset: "claude_code"}` object: `--tools=default` in the
  TypeScript dialect and `--tools default` in the Python one.
- A `StderrDelivery::Chunks` callback receives what Node's `StringDecoder` would, so a character split across reads
  arrives whole; `Lines` counts the partial-line limit in characters and trims like `str.rstrip()`.
- With a `session_store`, `SessionPersistence::Disabled` is refused in the TypeScript dialect with the upstream
  `sessionStore cannot be used with persistSession: false` error.
- `debug_stderr` is accepted and ignored, as Python no longer reads it.
- The inbound line limit `limits.max_buffer_size` is unset by default, as
  TypeScript keeps every line; `Limits::python()` sets Python's 1 MiB
  (`max_buffer_size`, where zero is valid). A longer line, counted in characters without its
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
- `WebSocketTransport` is the browser SDK's WebSocket transport as a `Transport`,
  for `connect_with_transport`: `WebSocketOptions { url, headers, header_delivery,
  auth_message }` with the `ws://` or `wss://` scheme only, a 15000 ms connect
  timeout, the auth line first, a `keep_alive` line every 50000 ms while the
  driver reads, newline-delimited inbound text frames and verbatim writes. Like
  the browser class it ignores `headers`; `HeaderDelivery::Handshake` sends them
  on the upgrade request instead. Cancelling the token given to `with_abort`
  closes the socket and fails the read with `WebSocketError::Aborted`.
- `SseTransport` is the browser SDK's SSE transport as a `Transport`, for
  `connect_with_transport`: `SseOptions::new(stream_url, send_url, session_id)`
  plus `headers`, `from_sequence_num`, `on_catch_up_truncated` and
  `on_delivery_update`. It streams with `Last-Event-ID`, treats 401, 403 and 404
  as permanent, reconnects other failures with a doubling backoff for five
  attempts in a row, gives the stream 15000 ms to answer and 45000 ms of
  silence, and POSTs writes in order with a 30000 ms timeout, honouring
  `Retry-After` and dropping a line after three failures. `SseTransport::handle`
  returns an `SseHandle` with `suspend`, `resume`, `expect_control_response`,
  `drop_counts`, `last_sequence_num`, `ccr_event` and `mark_delivered`. Cancelling the token
  given to `with_abort` fails the read with `SseError::Aborted`.
- `ClientHandle::apply_flag_settings` is TypeScript `applyFlagSettings`: the
  settings object goes to the CLI as given, so a `null` value clears that flag
  setting. `ClientHandle::update_settings` is TypeScript `updateSettings`, with
  `SettingsUpdateSource::UserSettings` or `SettingsUpdateSource::LocalSettings`
  naming the file the CLI writes. Python has neither control.

Model strings and paths are stored without extra identifier restrictions.
`env` values are `ConfigValue`s, which convert from strings as `Sensitive`;
debug output lists every variable name, shows values the caller marks `Plain`,
and prints sensitive values as `<redacted>`. MCP stdio arguments and
environment values and HTTP or SSE header values follow the same rule.
`ModelName` keeps the model out of debug output, so options debug output shows
every other setting without listing fields by hand.
A Windows `.bat` or `.cmd` CLI path starts, as in TypeScript;
`WindowsBatchCli::Refuse` selects Python's refusal at process start, which
fires only on Windows, as in Python.
Python exposes [`connect`](https://github.com/anthropics/claude-agent-sdk-python/blob/22b8eb9c19adfa78759b79e22bd664a57843bb06/src/claude_agent_sdk/client.py#L93)
and [`disconnect`](https://github.com/anthropics/claude-agent-sdk-python/blob/22b8eb9c19adfa78759b79e22bd664a57843bb06/src/claude_agent_sdk/client.py#L607)
on an existing client; Rust's `connect` is the async factory that returns the
connected client, and `disconnect(self)` consumes it, so a second disconnect
cannot be written rather than being a no-op. TypeScript's `Query.close()` is
`close()` on the query, the client or a handle. Python's `get_server_info()` is
`ClientHandle::initialization_result()`.
`ClaudeSdkClient::new(options)` keeps Python's construct-then-connect shape: the
`UnconnectedClient` it returns takes a transport or adapters and then
`connect(prompt)` with no prompt, a string, or a stream that a background task
writes, through the same factories. `scope(body)` is `async with`: it connects,
runs the body and always disconnects, returning `Error::Cleanup` when both fail.
`query` writes a string prompt in Python's `"default"` session under
`ArgvStyle::Python` and in the empty session `sdk.mjs` writes otherwise;
`query_in_session` names the session, and `query_stream_in_session` sets it only
on inputs that name none.

Options include programmatic agents, model/effort, tool rules, budget, output
schema, checkpointing, environment, additional directories, resume and fork.
`OutputSchema::new` and `UsdBudget::new` keep any value and pass it on, as both
upstream SDKs do; `OutputSchema::validated` and `UsdBudget::checked` opt in to
rejecting nonlocal references and nonfinite or negative amounts.
`UsdBudget::from_integer` keeps a Python `int` amount, which the Python SDK sends
as `str(0)`, `0`, where the float `0.0` goes as `0.0`. Rewind requires
checkpointing and CLI support.

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
only when a hook is set to block on failure, and a run-end wait read from
`CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` the way TypeScript reads it (600 seconds
when unset, zero for no limit). `Limits::python()` selects the
Python SDK's values: a 1 MiB line cap, a 60 second control wait, a 100 message
buffer that stops reading stdout while full, and its initialize wait, shutdown
ladder and `int()` parse of that variable. `RunEndTimeout::Fixed` opts into a
fixed run-end wait. Every other bound (pending controls, inbound callbacks, tracked
tasks, queued inputs, a failing message queue, command queue room) is opt-in and
keeps its typed error. A CLI that exits with status 0 before a result ends the
messages quietly, as both SDKs do; `EndWithoutResult::Fail` reports it as
`Error::UnexpectedEof` instead. Stderr debug output omits its contents. `QueueCapacity`
validates Tokio's range; `Timeout::checked` rejects zero and values past 24
hours.

## Settings

`Settings` is generated from the pinned TypeScript declarations: every declared key is a
typed field, and keys it does not declare are kept in order. `Settings::from_json` then
`to_json` reproduces the input byte for byte. Each value under a key the declarations do not
list, in `Settings` and every other generated type, is a `DeepJsonValue`: a `serde_json::Value`
that drops, clones, compares and prints with loops, so a member nested 9997 levels deep needs no
more than the 2 MiB stack of a spawned thread. A generated type writes such a member as the
`Value` would, but past 64 levels as its JSON text, which `serde_json::to_value` reads back only to
128 levels; `DeepJsonValue::encode` reads it back at any nesting, and `DeepJsonValue::to_json`
writes a lone value at any nesting. A `DeepJsonValue` on its own serializes and deserializes
exactly as the `Value` it holds. `SettingsInput` names the three forms the
`--settings` value takes: `Path`, `Json` and `Inline(Settings)`. Nothing is validated.
`SandboxSettings` and its parts, `ResolvedSettings`, `ProvenanceEntry`, `ResolvedSettingSource`,
`PolicySettingsOrigin`, `ResolveSettingsOptions` and `SDKSettingsParseError` carry the upstream
names; Python sandbox dicts decode and encode through the sandbox types unchanged.
`ResolveSettingsOptions` holds `managed_settings` and `server_managed_settings` as untyped
`DeepJsonObject`s, because `resolveSettings` takes them as raw objects and reads them key by key
through the policy schema, keeping a key the `Settings` type would refuse out of the result
instead of refusing the call.

`resolve_settings` ports TypeScript `resolveSettings`: the user, project, local and flag sources
cascade with array union, and the managed policy comes from the first tier that has settings
(remote, macOS plist via `plutil`, Windows registry via `reg query`, `/etc/claude-code`, the
parent process). The result carries per-key provenance and `machinePolicy.didNotLoad`.
`resolve_settings_with` runs it against any `SettingsHost`, so tests inject files, variables and
command output. A `ResolverState` holds what upstream keeps between calls;
`ResolverState::with_rereads(PolicyRereads::Inline)` runs machine policy re-reads on the calling
thread, so the host's clock alone decides when one has ended. Upstream reads the launch directory
when the SDK module is imported; here the loader reads it when the binary that links the crate
loads, before `main`, and `adapters::capture_launch_directory()` returns that reading. `filter_escalating_default_mode` ports `filterEscalatingDefaultMode`. The
`claude-sdk:resolve-diff` mise task compares the output with the pinned TypeScript resolver
byte for byte. Numbers read as `JSON.parse` reads them: integers above 2^53 round, and a number
past the double range reads as `Infinity`. A typed setting refuses `Infinity`, an `env` value spells
it `"Infinity"` (env values go through `String()`, as `z.coerce.string()` does), and the result
holds `null` where it survives, which is what `JSON.stringify` prints for it. Gap: schema value
normalization.

The Python dialect reads every settings file as Python's `json.loads` reads it: integers of any
size, floats that stay floats (`1.0` is not `1`), and `NaN`, `Infinity` and `-Infinity`.
`resolve_python_settings`, `resolve_python_settings_with` and `resolve_python_settings_with_state`
return the result as a `PythonJsonValue`, where each integer is an `ExactInteger` and each float an
`f64`, and `python_json_dumps` prints it as `json.dumps` does. `resolve_settings_in_dialect` and
`resolve_settings_in_dialect_with` with `SettingsDialect::Python` return the typed
`ResolvedSettings` instead, which holds integers up to the `u64` and `i64` range exactly, a double
for larger ones and `null` for the non-finite floats. `python_json_loads` and `python_json_dumps`
are checked against CPython over a corpus of doubles, documents and refused text
(`.config/mise/lib/claude-sdk-freshness/python_json_goldens.py` writes the goldens).

A settings file in the Python dialect nests as deeply as `json.loads` lets it where the Python SDK
runs: 9997 arrays and objects from a plain function (`PythonCStack::MODULE`, the default), 9993
under `ClaudeSDKClient.connect` (`PythonCStack::CLIENT_CONNECT`) and 9987 under `query()`
(`PythonCStack::QUERY`). `ResolverState::with_python_c_stack` picks one. The Python dialect runs
on a thread of its own whose stack holds a walk that deep; a result nested thousands of levels
deep needs a stack that size wherever it is dropped, compared or printed, except under a key the
declarations do not list (`.config/mise/lib/claude-sdk-freshness/python_settings_file_goldens.py`
records each boundary).
Its bytes decode as `Path.read_text(encoding="utf-8-sig")` decodes them in the Python SDK's
`_build_settings_value`: one leading byte order mark is dropped, `\r\n` and a lone `\r` become `\n`,
and a file that is not UTF-8 is skipped like a file `json.loads` rejects. The `--settings` value
the Python dialect builds reports such a file with CPython's `UnicodeDecodeError` message
(`.config/mise/lib/claude-sdk-freshness/python_settings_goldens.py` records the messages).
A string or key holding a lone surrogate escape such as `"\ud800"` keeps it, as `json.loads` does:
`resolve_python_settings` returns the exact `str`, and `ResolvedSettings` holds U+FFFD for each
lone surrogate. Inside the engine such a string stands as the same text with each lone surrogate
moved to a private-use code point, so it measures and matches like the surrogate. The resolve
first reserves every private-use code point its inputs hold (settings texts, escapes included,
managed settings, settings paths and the working directory) and picks stand-ins only among the
rest, so a private-use code point read anywhere in the resolve stays as it is. The TypeScript
dialect reads a settings file as the pinned resolver does on Node: UTF-16LE when it starts with
the bytes FF FE, UTF-8 with U+FFFD for each invalid sequence otherwise, one leading byte order
mark stripped before `JSON.parse`. It keeps lone surrogates, escaped or decoded from UTF-16LE,
with the same stand-ins, and `ResolvedSettings` holds U+FFFD for each one, while
`resolve_settings_json_with_state` returns the text `JSON.stringify` prints for the object
`resolveSettings` resolves, each lone surrogate escaped as `\udxxx`.
`resolve_settings_exact` and `resolve_settings_exact_with_state` return that object as a
`PythonJsonValue` whose strings and keys keep every lone surrogate and whose numbers hold the
doubles `JSON.stringify` prints, and `js_stringify_exact` prints it as `JSON.stringify` does.
`resolve_settings_exact_in_dialect_with_state` returns it for `SettingsDialect::TypeScript`, the
default, and the `resolve_python_settings` value for `SettingsDialect::Python`
(`.config/mise/lib/claude-sdk-freshness/ts_settings_file_goldens.py` records the cases).
The settings schema drops each `__proto__` member of the document, of a schema object and of a
record, as zod's assignment does, and keeps one inside a value it passes through whole.
The files merge as lodash `mergeWith` merges them: an array or object under `__proto__` merges
nothing, a scalar there is written, and members merge in `for...in` order, index-like keys first.
An array merged into an array-like object, one whose `length` is a whole number from 0 to
2^53 - 1, first turns it into an array of that length holding its index members, and an object
merged into an array sets its elements, its `length` and named properties no later step prints.
Each hole or `undefined` element prints as `null`; a `length` that is no valid array length
rejects with `RangeError` (`ResolveSettingsError::InvalidArrayLength`), and more holes than a V8
string can print reject with `ResolveSettingsError::InvalidStringLength`
(`.config/mise/lib/claude-sdk-freshness/ts_settings_merge_goldens.py` records the cases).
A settings document in the TypeScript dialect nests at most `SETTINGS_DEPTH_BOUND` (500) levels, counting a
scalar as 0 and `{}` or `[]` as 1. The pinned `resolveSettings` copies and merges a document recursively until V8
runs out of stack, and where that happens depends on the tier V8 compiled each function to, which functions are
compiled, the caller and the history of the process, so the SDK picks one bound that is the same everywhere. A
resolve that reads or receives a document nested that deep, whichever one (a user, project or local settings file,
`managed-settings.json` or a drop-in, a policy from the registry or a property list, or the `managedSettings` and
`serverManagedSettings` options), fails with `RangeError` (`ResolveSettingsError::MaximumCallStack`) and drops
nothing, whatever the host, the `ResolverState` or what resolved before. Below the bound the answer is exactly the
one of Node, merge rules included. The bound is the largest multiple of 100 at most half the smallest margin any
recorded Node reaches: Node 24.13.1 on Windows x64 answers as the reference to a document 1190 levels deep and
differently from 1191 (CI run 44055880748, job 50), half of which is 595, so the bound is 500. A document nested
past the 128 levels `serde_json` reads is still read below the bound
(`.config/mise/lib/claude-sdk-freshness/ts_settings_depth_goldens.py` and
`.config/mise/lib/claude-sdk-freshness/ts_managed_settings_goldens.py` record what Node answers).

```rust,no_run
use trogon_claude_agent_sdk::{ResolveSettingsOptions, python_json_dumps, resolve_python_settings};

async fn python_text(options: &ResolveSettingsOptions) -> Result<String, Box<dyn std::error::Error>> {
    Ok(python_json_dumps(&resolve_python_settings(options).await?))
}
```

To print resolved settings exactly as the TypeScript SDK would, serialize them with `js_stringify`
instead of `serde_json::to_string`. It writes what `JSON.stringify` writes: the shortest digits of
each double in fixed notation from 1e-6 up to 1e21 and exponent form outside it (`serde_json`
switches to exponent form for whole numbers from 2^64 up and for fractions below 1e-5), negative
zero as `0`, array-index keys first, and the same string escapes:

```rust,no_run
use trogon_claude_agent_sdk::{ResolveSettingsOptions, js_stringify, resolve_settings};

async fn effective_text(options: &ResolveSettingsOptions) -> Result<String, Box<dyn std::error::Error>> {
    let resolved = resolve_settings(options).await?;
    Ok(js_stringify(&serde_json::to_value(&resolved.effective)?))
}
```

## External session stores

Set `session_store` to a `SessionStoreOptions`. Implement `SessionStore` for a
backend or use `InMemorySessionStore`. Entries are keyed by project/session and
optional subagent path; the in-memory backend deduplicates stable UUIDs.
[The store example](examples/session_store.rs) needs no Claude or network.

As upstream, `SessionKey::new`, `SessionProjectKey::new` and
`SessionSubpath::new` take any text, and `SessionKey::from_transcript_path`
resolves a relative path or projects directory against the working directory
and validates nothing in the path beyond its shape. On Windows the transcript
path compares case-insensitively, accepts both separators, drives and UNC
shares, and keeps the case of the names it returns. Upstream checks a subpath
only where resume turns it into a file, and each SDK checks it its own way, so
resume applies the rule of the selected dialect. `SessionKey::checked`,
`SessionProjectKey::checked` and `SessionSubpath::checked` are the opt-in
constructors that reject an id, project key or subpath that is not a safe file
name component, with typed errors; `SessionSubpath::checked` reads a drive
prefix as the platform does (any character before a colon on Windows, a letter
on POSIX) and only on Windows also rejects empty components and reserved device
names. On Windows Python reads a part such as `a:x` as a drive, so
`SessionKey::from_transcript_path_with` and the mirror give a Python-dialect
store the key CPython 3.12 derives; TypeScript is the default.
`SessionStoreOptions::project_key` is an opt-in override that defaults to `None`,
which derives the key from the working directory exactly as upstream does
(TypeScript's `CLAUDE_CODE_PROJECT_DIR_NAME` is its only upstream analogue). A
key you set is joined to the projects directory as given, so build it with
`SessionProjectKey::checked` when it does not come from trusted code.

Mirroring follows CLI local writes. Each result waits for its own store barrier
while controls remain serviceable. Failures appear as `mirror_error` messages
and never fail the run; a transcript path outside the projects directory is
logged and dropped. Pending entries are coalesced per key and drained early when
they pass the entry or byte budget; frames are never rejected. A failed append
is retried twice with 200 ms and 800 ms backoff, for any backend error; timeouts
are not retried because an in-flight write may still complete. As upstream,
every failure is reported and a flush waits as long as its appends take, each
bounded by `append_timeout`. `SessionStoreLimits::max_retained_failures` opts
into folding the overflow into one suppressed-count failure, and
`SessionStoreLimits::flush_timeout` opts into a flush deadline that fails with
`SessionError::MirrorFlushTimeout`.

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
with a session store. As upstream, a relative configuration or home directory
is used as given and store loads are unbounded;
`SessionStoreOptions::require_absolute_config_paths` and the resume limits in
`SessionStoreLimits` opt into typed errors instead. Without a home directory,
Python fails the connect with `SessionError::MissingConfigDirectory`, while
TypeScript, which finds the projects directory per mirror frame, connects and
drops each frame with `MirrorEnqueueError::MissingConfigDirectory`; a store
resume without a configured directory needs it in both.

Project-key sanitization uses the pinned TypeScript SDK’s UTF-16 replacement
behavior. Native hashing can differ for long project paths; use an explicit
`SessionStoreOptions.project_key` when
necessary. Backend retention and cross-process coordination belong to the backend.

## Compatibility and verification

The references are Python SDK
[`0.2.181`](https://github.com/anthropics/claude-agent-sdk-python/tree/22b8eb9c19adfa78759b79e22bd664a57843bb06)
and published TypeScript SDK `0.3.323`, paired with CLI `2.1.323`.
Every feature in [the freshness mapping](.config/mise/lib/claude-sdk-freshness/mapping.json)
is supported, including the TypeScript `browser`, `bridge` and `extract`
entrypoints (`browser_query`, `attach_bridge_session` and `extract_from_bunfs`).
Embedded MCP servers answer `initialize`, `ping`, `tools/list` and `tools/call`;
they are not general-purpose MCP servers.
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

# @trogonai/sandbox-agent-tools

An LLM-friendly tool bundle over one `Sandbox` from `@trogonai/sandbox-ws-client`. The `trogon-sandbox-mcp` tools mirror the wire types (a lease id on every call, argv vectors, base64 payloads, a required timeout). These tools do not: inputs are flat, payloads are text, and every result starts with a `Verdict:` line.

Requires Node >= 22.7. The only runtime dependencies are `@trogonai/sandbox-ws-client` and `zod`. No agent framework is imported.

## Usage

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';
import { createSandboxTools, nativeFileOps, toolList } from '@trogonai/sandbox-agent-tools';

const client = await SandboxWsClient.connect({
  gatewayHttpUrl: 'http://localhost:8080',
  gatewayWsUrl: 'ws://localhost:8080',
  apiKey: process.env.SANDBOX_API_KEY!,
});
const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });

const tools = createSandboxTools(sandbox, {
  cwd: '/workspace',
  allow: ['run_command', 'read_file', 'edit_file', 'find_files'],
  nativeFileOps: nativeFileOps(client, sandbox.sandboxId), // optional, see below
});

const out = await tools.run_command!.execute({ command: 'npm test' });
// Verdict: failed (exit 1)
// stdout:
// ...
```

## Tools

| Name                 | Inputs                                                         | Use it for                                                                |
| -------------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `run_command`        | `command`, `cwd?`, `timeout_seconds?`                          | Shell commands that finish                                                |
| `read_file`          | `path`, `offset?`, `limit?`                                    | Numbered, line-paged text                                                 |
| `write_file`         | `path`, `content`                                              | Create or overwrite, creating parents                                     |
| `edit_file`          | `path`, `old_text`, `new_text`, `expected_occurrences?`        | Exact-text replace, refused on a count mismatch                           |
| `find_files`         | `path?`, `pattern?`, `glob?`, `recursive?`, `max_results?`     | List files, or search contents for literal text                           |
| `background_process` | `action`, `name`, `command?`, `cwd?`                           | Start, check or stop a long-running process                               |
| `start_server`       | `name`, `command`, `port`, `path?`, `cwd?`, `timeout_seconds?` | Start a dev server, wait until it answers, return its preview URL         |
| `preview_url`        | `port`                                                         | Resolve the address of a port in the sandbox                              |
| `checkpoint_files`   | `path?`, `hash?`                                               | Record a directory as a baseline and return its id                        |
| `file_changes`       | `since?`, `path?`                                              | Files added, modified and deleted since a checkpoint or the sandbox start |
| `watch_files`        | `path`, `recursive?`, `duration_seconds?`, `max_events?`       | Collect file events for a bounded time                                    |

Each description also says when not to use the tool. `toolSchemas` and `toolDescriptions` export the zod schemas and text on their own, for example to build JSON Schema with `z.toJSONSchema`.

Every `execute` takes raw model input, validates it with the zod schema and returns text. Bad input and failed operations come back as `Verdict: error, ...` instead of throwing.

## Output handling

- The bundle asks the stack to cap each stream (`max_output_bytes`, default 64 KiB). When the stack drops the middle it reports `truncated`, and the result carries a note.
- Text sections are clipped again to `maxOutputChars` (default 30000), keeping head and tail.
- Output is decoded as UTF-8. Invalid bytes become U+FFFD and the result says so. `read_file` refuses binary files.
- A command killed at its deadline reports `Verdict: timed out after Ns and was killed`.

## Options

`allow` (subset of tools), `deny` (tools to drop, wins over `allow`), `beforeRun` (host policy hook that can refuse a call or rewrite its shell command), `cwd` (working directory and base for relative paths), `defaultTimeoutSeconds`, `maxTimeoutSeconds`, `maxOutputBytes`, `maxOutputChars`, `defaultReadLines`, `previewScheme`, `nativeFileOps`, `maxWatchSeconds` (cap on `watch_files`, default 30), `streamIntervalMs` (gap between streamed progress snapshots, default 250).

## File operations

Without `nativeFileOps`, `edit_file` reads and rewrites the file (not atomic), `find_files` shells out to `ls`, `find` and `grep`, and `write_file` runs `mkdir -p` first. With `nativeFileOps(client, sandbox.sandboxId)` the tools use the stack's `file_op` verbs (atomic exact-once edit, bounded search and listing). A provider that answers `unsupported` falls back to the shell path automatically.

## Background processes

`background_process` keeps a registry of detached commands in the bundle, keyed by name. Status reports `running` or the exit code with output. Output is only available once the process exits, and the registry lives in memory, so it does not survive a restart of the caller. The stack's named process verbs (`process` on a run request, `listProcesses`, `waitReady`, `killProcess`) are not used yet.

## Change tracking and watching

`checkpoint_files`, `file_changes` and `watch_files` use `sandbox.fs.checkpoint`, `changes` and `watch`, which need a provider with change tracking and file watch. Without them the tool answers `Verdict: error, ...` as text. `watch_files` is bounded on purpose: it returns after `duration_seconds` (capped by `maxWatchSeconds`) or `max_events`, whichever comes first, and stops the watch. For the net result of a long task, checkpoint first and call `file_changes` after.

## Streaming command output

`run_command` can report progress while the command runs. Each tool in the set has an optional `stream(input)` that yields text snapshots (`Verdict: running` with the output so far) and ends with the same text `execute` returns. It starts the command with `streamOutput` and follows the stack's durable command log. When the stack keeps no durable logs it reads the client's live chunk feed instead until the command exits; output written before that feed attached shows up only in the final result. The Vercel AI SDK adapter consumes it with `toAiSdkTools(tools, { streamOutput: true })`, which makes `run_command` an async iterable: each snapshot is a preliminary tool result and the last one is what the model sees. The Mastra adapter does the same through the tool's `writer` with `toMastraTools(tools, { streamOutput: true })`, and falls back to the final text when the Mastra release passes no `writer`. The OpenAI Agents adapter returns the final text only, because a function tool there resolves to a single value and the SDK has no channel for partial tool output.

## Framework adapters

The adapters are plain mapping functions that return the objects each framework's own helper takes. They do not import the framework, so installing this package never pulls one in.

```ts no-check
import { toAiSdkTools } from '@trogonai/sandbox-agent-tools/ai-sdk';
const result = await generateText({ model, tools: toAiSdkTools(tools), prompt });

import { toMastraTools } from '@trogonai/sandbox-agent-tools/mastra';
const mastraTools = Object.fromEntries(toMastraTools(tools).map((t) => [t.id, createTool(t)]));

import { toOpenAiAgentsTools } from '@trogonai/sandbox-agent-tools/openai-agents';
const agent = new Agent({ name: 'dev', tools: toOpenAiAgentsTools(tools).map((t) => tool(t)) });
```

These shapes follow the Vercel AI SDK v5 (`inputSchema`), Mastra (`createTool` with `id`) and OpenAI Agents JS (`parameters`). They are tested for shape only, since the frameworks are not installed here.

## Not yet covered

- Aborting a framework call does not kill the running command.
- No command veto or root confinement.
- No secret redaction of output.

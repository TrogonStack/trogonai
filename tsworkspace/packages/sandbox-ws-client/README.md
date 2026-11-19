# @trogonai/sandbox-ws-client

TypeScript client for a separately deployed compatible sandbox WebSocket gateway. It exposes a low-level `SandboxWsClient` plus a `Sandbox` wrapper around a single lease. Gateway, broker and provider availability are deployment prerequisites; this package contains the client and an in-memory test double.

Requires Node >= 22.7 (or any runtime with a global `WebSocket` and `fetch`).

## Usage

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';

const client = await SandboxWsClient.connect({
  gatewayHttpUrl: 'http://localhost:8080',
  gatewayWsUrl: 'ws://localhost:8080',
  apiKey: process.env.SANDBOX_API_KEY!,
});

try {
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId('acme'),
    providerPref: ['local_docker'],
    timeoutMs: 5 * 60_000,
  });
  try {
    // Run a command to completion.
    const result = await sandbox.runCommand('sh', ['-c', 'echo hello']);
    result.throwIfFailed();

    // Write and read files.
    await sandbox.writeFiles([{ path: '/tmp/greeting.txt', content: 'hi' }]);
    if ((await sandbox.readFile({ path: '/tmp/greeting.txt' })) !== 'hi') throw new Error('file read did not match');
  } finally {
    await sandbox.delete();
  }
} finally {
  client.close();
}
```

Always release the lease in a `finally`. A lease that is not deleted keeps running until its timeout.

## Scope, cancellation and defaults

- Use `try/finally` to release a lease on native Node 22. `sandbox[Symbol.asyncDispose]()` stops a named sandbox and deletes an unnamed one; repeated disposal does nothing, and a failed release can be retried. The `await using` syntax requires a newer runtime or a compiler that transforms explicit resource management; Node 22's native TypeScript loader does not parse it.
- `runCommand({ signal })` requires advertised `command_cancellation`. Providers also advertising `command_launch_cancellation` retain a launch reference before dispatch, so abort can be reconciled even when the start reply is lost. Named signal requests require both capabilities and preserve reused or unknown executions. A foreground call obtains an early detached execution identity and waits for its result. Unsupported or unknown capability is refused before launch. An already aborted signal sends nothing.
- Commands run with `CI=1`, `DEBIAN_FRONTEND=noninteractive`, `GIT_TERMINAL_PROMPT=0` and `PAGER=cat` merged under your `env`, so tools that prompt fail or proceed instead of hanging. Your `env` wins per variable; pass `nonInteractive: false` to send exactly your `env`. Runs with `stdin: 'interactive'` get no defaults. The list is exported as `NON_INTERACTIVE_ENV`.
- `Sandbox.create` takes typed `ports` (sent as `exposed_ports`) and `secrets` (sent as `secret_bindings`), validated before any lease is requested.
- Error messages and causes are scrubbed of URL credentials, bearer tokens, `token=`-style query parameters, API-key and authorization headers and JSON secret fields. `scrubSecrets` is exported. This is a best-effort filter over known shapes, so do not rely on it to make arbitrary text safe to share.

## Attaching, ownership and cleanup

- `Sandbox.connect(client, sandboxId)` attaches to an existing lease by id. It scans the lease list (there is no get-by-id verb) and throws `SandboxNotFoundError` (code `lease_not_found`) when nothing matches.
- `Sandbox.create` tags leases with `client=sandbox-ws-client` so `Sandbox.list` and `Sandbox.cleanup` only touch leases this client created. Pass `includeForeign: true` to see everything, or `ownershipTag: false` on create to skip the tag. The tag uses one of the five tag slots and is skipped when all five are taken or you set `client` yourself. Leases created before the tag existed, and forks, need `includeForeign: true`.
- `Sandbox.cleanup(client, { tags, olderThanMs, idleForMs, dryRun, includeForeign })` reaps matching leases.

## Command results

`CommandFinished` carries `ok` (exit code 0 and not timed out), `timedOut`, `truncated` and, for synchronous runs, a client-measured `durationMs`. Helpers: `text()`, `lines()`, `json<T>()` and `throwIfFailed()`, which throws `CommandFailedError` with the exit code and stderr.

## Named processes

`sandbox.start({ cmd, args, name, cwd, env, readiness, readyTimeoutMs })` gets or starts a named detached process. A repeated running name returns the existing execution; its command, environment and readiness are retained. Without `name`, a stable hash of the command and arguments supplies a valid name. Changing only `cwd` or `env` does not select another instance; supply distinct names for separate instances. Readiness accepts the worker's `tcp_port` or literal `output_contains` probe and waits for at most five minutes. Use `startServer` when you need HTTP path checks and preview setup.

`sandbox.process(name)` finds a process from another client handle. `sandbox.processes()` returns the worker's execution IDs, names, states, exit codes and recorded probes, without inventing command text, PIDs or timestamps. A running named execution takes precedence; when only multiple exited executions share a name, lookup refuses to guess their order. Select an execution from the inventory instead.

The returned `SandboxProcess` exposes `name`, `command`, `wait`, `stop`, `restart`, `logPage` and `logs`. Restart uses the backend's recorded specification, so it works after reconnect when the worker supports it. Named readiness failures and cancellation preserve a potentially reused process; stop it explicitly when that is your intent. The memory client models named reuse, discovery and explicit restart, and refuses readiness probes and durable logs that it does not model.

## Structured files

`sandbox.fs.list(path, { recursive, maxEntries })` returns `{ entries, truncated }`. `sandbox.fs.edit(path, { find, replace, expectedOccurrences })` performs an atomic exact-text edit; the default expected count is one and a mismatch throws `EditRejectedError` without writing. `sandbox.fs.search(query, { path, glob, maxResults })` returns `{ matches, truncated, filesScanned }` for a literal text query. Search defaults to `/`; supply `path` to limit its scope. Listing and search retain the provider's paths, which the local worker reports relative to the requested directory, and expose truncation so a partial result stays visible. These methods use structured file operations and do not require shell tools.

The memory client models metadata, exact edits and literal searches, including `*`, `**` and `?` globs. It explicitly refuses regex searches, additional glob syntax and filesystem user overrides. Real workers determine their own support. Both inputs and structured replies are validated.

## Shell template

```ts no-check
const result = await sandbox.$`git clone ${url} ${dir}`;
```

Interpolated values are single-quoted for `sh`, arrays expand to one word per element, and `raw(text)` opts out of quoting. `null`, `undefined`, objects and NUL bytes throw. `sandbox.shellWith(params)` takes the same script with run options.

## Network allow list

```ts
await Sandbox.create(client, { tenant, network: { allow: ['registry.npmjs.org', '*.pythonhosted.org'] } });
```

Builds a deny-by-default policy with a pass-through rule per host. An empty list means deny all. Entries must be hostnames (optionally with a port or a leading `*.`), not URLs. `network` and `networkPolicy` are mutually exclusive on create; `updateNetworkPolicy` accepts `{ allow }` too.

## Retry

Idempotent requests (reads, `sandbox_lease`, `sandbox_reap_leases`, `sandbox_update_tags`) are retried on retryable errors with exponential backoff and full jitter: 3 attempts, 100 ms base, 2 s cap. A `retry_after_ms` (or `retry_after_seconds`) hint from the broker is honored, exposed as `SandboxWsError.retryAfterMs`, and the request fails fast when the hint exceeds `maxRetryAfterMs` (30 s). Configure with `retry: { maxAttempts, baseDelayMs, ... }` on the client options, or `retry: false` to disable. Subscriptions and file operations are never retried.

## Deadlines and cancellation

`command.cancel({ timeoutMs })` returns `CommandFinished` after the exact owned execution is confirmed completed. Concurrent callers share one cancellation. When an attached signal aborts, `command.wait()` rejects with `CommandCancellationError`. A normal cancellation with `cleanupPending: false` is confirmed; `cleanupPending: true` and `outcomeUnknown: true` mean termination has not been established. A reused named command instead reports `code: 'command_preserved'` and `preserved: true`, which confirms preservation rather than process completion. Unknown named ownership never grants permission to stop it. The error carries the known `executionId`, any retained `launchKey`, and the original signal reason as its cause. Signal delivery or a missing execution record alone does not prove completion.

`runCommand({ signal, cancellationTimeoutMs })` bounds acknowledgement and cleanup waiting. The default is 10000 ms; accepted budgets are integers from 1 to 60000 ms. With `command_launch_cancellation`, the request carries a fresh `launch_key` and `require_launch_cancellation: true`; abort uses an independent control request when the start acknowledgement is unavailable. A lost cancellation reply is retried with the same reference within that budget, respecting server retry delays. Repeat `client.cancelCommandLaunch(sandboxId, error.launchKey)` to reconcile an incomplete result or a start transport failure. Frozen transport errors are wrapped with the original classification and sanitized cause when a reference cannot be attached directly.

The control reply is `canceled` after confirmed owned cleanup or prevention of launch, `preserved` for an existing execution, `pending` when the outcome is unresolved, or `unsupported`. A receipt does not fabricate retained command output. Ordinary command requests do not require this stronger protocol. Providers with only acknowledged-command cancellation can still clean up after a late trusted reply, but a permanently lost acknowledgement remains unresolved there. `Command.cancel` uses the same default or the signal-bound command's configured budget. The memory client does not advertise verified command cancellation because a scripted handler can outlive its simulated result.

`Sandbox.create` and `Sandbox.fromSnapshot` with `signal` first check `client.operationCapabilities()`. A broker without `supports_create_cancellation` is refused before the lease request. An accepted request carries a fresh operation id, and abort records cancellation against that id through an independent control request. `OperationCancellationError` exposes `operationId`, `canceled` and `cleanupPending`, with the original signal reason as its cause. A confirmed cancellation has `canceled: true`; pending cleanup has `outcomeUnknown: true` and remains observable through `client.getOperation(operationId)`.

Signal-bound creation requests `client.cancelOperation(operationId, { compensateCreated: true })`, so a broker can retire a newly-owned allocation whose creation finished before its reply arrived. This requires broker-recorded allocation ownership and exact cleanup confirmation; a reused lease is preserved. Compensation keeps the original operation outcome immutable: a successful create can still have `state: 'succeeded'` while `compensation` reports `pending` or `cleaned`. A plain `client.cancelOperation(operationId)` does not opt into compensation of a completed create.

When cancellation cannot retire an already-completed create, the SDK reports `operation_already_completed` with the known `leaseId`. Inspect the operation status and actual lease before deciding what to do next. For pending compensation, repeat `client.cancelOperation(operationId, { compensateCreated: true })` to retain that intent. Abort after creation has returned does nothing. Creation's `cancellationTimeoutMs` uses the same bounds and default as command cancellation. The memory client reports create cancellation unsupported. Cancellation of other in-flight lifecycle operations remains outside this contract; these APIs do not establish full cancellation parity across providers.

`sandbox.waitUntilReady({ ports, timeoutMs })` resolves once the lease is running and every listed port (at most 16) accepts a connection inside the sandbox, and throws `TimeoutError` naming the ports that never answered. Port checks need a provider that can exec and an image with `bash` or `nc`; otherwise they return `unsupported`.

`sandbox.startServer({ cmd, args, port, path, timeoutMs })` starts a process in the background and resolves `{ command, url, expiresAt }` once the port accepts connections (and `path` answers over HTTP, when given), where `url` is the preview URL. The broker checks the port using `bash` or `nc` in the image; the optional path check needs `curl` or `wget`. If the process exits first it throws `ServerExitedError` carrying the process output; on timeout it throws `TimeoutError` with recent output when the broker keeps durable logs. Pass `preview: false` to skip the URL. `resume()` and `wake()` restart registered servers whose process is gone and refresh their URLs. `server.restart()` restarts on demand, `server.stop()` forgets the server, and `restartOnResume: false` opts out. `MemorySandboxClient` models no ports: script readiness and preview responses in tests.

Readiness or preview failure triggers cleanup of an unnamed server or a named server with provider-confirmed `origin: 'created'`. Named `origin: 'reused'` and legacy or unknown ownership are preserved. `command.origin` exposes that creation evidence. Extensible errors carry `executionId` for intentional cleanup; frozen readiness errors and primitive thrown values retain their original identity. Explicit `server.stop()` and `server.restart()` still act on the returned process.

A failed multi-server restoration rolls back its own new unnamed executions and confirms termination before reporting their cleanup complete. It preserves adopted and potentially reused named processes. Extensible primary errors carry `restoreCleanup.stoppedExecutionIds` and `restoreCleanup.incompleteExecutionIds`. When termination remains unconfirmed or cleanup context is unavailable, inspect or stop the affected executions before retrying.

Snapshot receipts include a bounded `clientState` restart plan for registered servers. `Sandbox.fromSnapshot` consumes it automatically and waits for readiness again. Environment values, signals, per-call cancellation budgets and literal redaction values are never saved in that plan: supply replacement environment values when restoring. Keep `exportClientState()` in your own trusted store to reconnect through `Sandbox.connect(client, id, { clientState, env })` or `Sandbox.get(client, { name, clientState, env })`. A name lookup alone does not recover another client's server intent. Known disk-only snapshots, including native Vercel, Modal and Fly snapshots, replay servers in a newly created sandbox. Memory-preserving or unclassified snapshots require authoritative process discovery; absent identity alone does not prove a process exited. See [Restore managed servers](docs/how-to/restore-managed-servers.md).

Snapshot retention is controlled by the broker. `snapshot({ expiresInMs })` rejects invalid values with `invalid_request` and valid per-snapshot expiry requests with `unsupported`, before capturing anything. This includes `0`, because the wire cannot promise indefinite retention for an individual snapshot.

Retire a provider-native receipt with `Sandbox.deleteNativeSnapshot(client, receipt.reference)`. It resolves `true` after deletion and `false` when prior deletion is confirmed. The authenticated gateway supplies the tenant; the reference is never a vendor deletion credential. Unsupported providers throw `unsupported`, and ownership or provider failures remain typed errors. `client.nativeSnapshotDelete(reference)` exposes the `deleted`, `not_found` and `unsupported` outcomes directly. Expired references can still be submitted for cleanup. `MemorySandboxClient` declares native deletion unsupported because it captures OCI-style snapshots only; those continue to use `Sandbox.deleteSnapshot(client, digest)`.

`preview(port)` returns the signed root URL for the exposed application. Resolve a relative application path with `new URL('settings', preview.url)` to retain its signed gateway prefix. `preview(port, { waitMs })` waits for the port to answer HTTP; `startServer({ path })` provides a separate guest-side HTTP path check.

`requestTimeoutMs` on the client options sets a deadline for every request, 10 s by default (`DEFAULT_REQUEST_TIMEOUT_MS`) to match the Rust client. A request with no reply by then rejects with `TimeoutError`, whose outcome is unknown: the gateway may still apply it. A command's `timeoutSeconds` (60 s when unset) and a long poll's `waitMs` are added on top, so they are not cut short. So are the gateway's own waits for the slow verbs when no `timeoutSeconds` is passed: 30 s for `lease`, `fork`, pause, resume and snapshot, 300 s for `waitReleased`, 30 s for `waitSandboxReady`. Raise `requestTimeoutMs` for a gateway that is slow to answer everything else.

The file transfer helpers (`uploadFile`, `downloadFile`, `uploadDir`, `downloadDir` and their verified forms) take a `signal` and stop between chunks when it aborts, rejecting with the signal's reason. A verified upload resumes from the bytes already written on the next call. `uploadFileStream` and `downloadFileStream` take or yield the file as a stream of byte pieces and hash it incrementally, so a file larger than memory never has to be held whole; they start at byte 0 instead of resuming, and a digest mismatch throws from the end of the stream. A read reply carries no digest, so pass `verifyPages: true` to `downloadFileStream` to check each page against a ranged checksum before it is yielded, at one extra round trip per page.

## Capabilities

`sandbox.capabilities` lists the features the provider reports for the lease (`fork`, `pause_resume`, `write_stdin`, and so on). `sandbox.supports('fork')` is a type guard that narrows the handle, and `capabilities.require('fork')` throws an `unsupported` error. Handles are populated only when the broker sends the capability list; otherwise `capabilities` is undefined and `supports` returns false. `withCapabilities()` sets them by hand.

`hasCapability(sandbox, name)` is the same guard as a free function, and also accepts the names other sandbox SDKs use (`pause`, `resume`, `snapshot`, `stdin`, `logs`, `watch`, `changes`, `domain`), mapped through `CAPABILITY_ALIASES`.

## Names shared with other sandbox SDKs

- `listSandboxes(client, { labels })` is `Sandbox.list` with `labels` for `tags`, and `reapSandboxes(client, { labels, ... })` is `Sandbox.cleanup`.
- `Sandbox.create(client, { trackChanges: true })` records a change-tracking baseline right after the lease starts (`{ path }` tracks one subtree), so `sandbox.fs.changes()` with no `since` reports everything changed since creation. When the baseline cannot be recorded, the new sandbox is stopped and the error thrown.
- `sandbox.fs.mark(name?)` records a checkpoint under a name and returns it; `changes({ since: name })` diffs against it. Marking again under the same name moves it. Marks live on the handle, so another handle to the same lease does not see them.

Deliberate differences:

- `changes()` reports `added`, `modified` and `deleted` (not `removed`) and carries no patch. Diff contents with `git diff` or by reading the files.
- There is no root-level `createSandbox` or `connectSandbox`; use `Sandbox.create` and `Sandbox.connect`.
- There is no `buildTemplate`. Images are OCI images built with your own tooling; snapshots and `fromSnapshot` cover reusable starting states.
- There is no `runConformance` in TypeScript. Providers plug in as workers that speak the worker contract over NATS, and the Rust conformance suite checks them.

## Browser apps: connect with a pre-minted token

`SandboxWsClient.connect` needs the tenant API key, which must never ship to a browser. Mint the session on your backend and hand the page only the short-lived token. The gateway's existing `POST /v1/sessions` endpoint does the minting; no other endpoint is involved.

Backend (any runtime with `fetch`), one call per signed-in user:

```ts
// POST /api/sandbox-session on your server
const response = await fetch(`${process.env.SANDBOX_GATEWAY_HTTP_URL}/v1/sessions`, {
  method: 'POST',
  headers: { 'X-Sandbox-API-Key': process.env.SANDBOX_API_KEY! },
});
if (response.status !== 201) throw new Error('session mint failed');
const { token, expires_at } = await response.json(); // expires_at is unix seconds
return Response.json({ token, expiresAt: expires_at, gatewayWsUrl: process.env.SANDBOX_GATEWAY_WS_URL });
```

Browser:

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';

const { token, gatewayWsUrl } = await (await fetch('/api/sandbox-session', { method: 'POST' })).json();
const client = await SandboxWsClient.connectWithToken({ gatewayWsUrl, token });
```

Things to know:

- The token carries the tenant's full access until it expires (the gateway's `--session-ttl-seconds`). The client cannot narrow it, so authenticate the user on your backend before minting and mint per user or per page load.
- There is no refresh. When the socket closes or the token expires, ask your backend for a new token and call `connectWithToken` again.
- A rejected handshake (expired or unknown token) throws `TransportError`. The token is scrubbed from error text, but do not log it yourself.

## Bundling

The package is ESM only and imports nothing but `ws` and `zod`. It needs `fetch` (for `connect`) and a WebSocket: the global `WebSocket` is used when it exists, and the `ws` package is the fallback.

- Node (>= 22.7): import it directly or bundle with `--platform=node`. A global `WebSocket` is present from Node 22, so `ws` is only a fallback.
- Browser: bundle with `--platform=browser`. `ws` resolves to its `browser` stub, which throws only if it is called, and the client never calls it while a global `WebSocket` exists. Use `SandboxWsClient.connectWithToken` so no API key is in the bundle.
- Next.js: use the client in a Route Handler or Server Action to mint tokens (see above), and in a `'use client'` component for `connectWithToken`. The package is plain ESM with no Node built-ins, so it should need no special config. This is not exercised under Next.js in this repo.
- Edge runtimes (Cloudflare Workers, Vercel Edge, Deno): they provide `fetch` and `WebSocket`, so bundle for the browser or worker target. The source uses no Node built-ins and `ws` is never reached. Also not exercised in this repo.
- Lambda and other Node bundles: `--platform=node` with `--format=esm` (or `--format=cjs`, the package has no top-level await).

Check a bundle with esbuild, for example:

```sh
esbuild src/index.ts --bundle --platform=node --format=esm --outfile=out/node.mjs
esbuild src/index.ts --bundle --platform=browser --format=esm --outfile=out/browser.js
```

Run the bundle checks against the runtime you deploy. esbuild is not a declared dependency of this package.

## API surface

- `SandboxWsClient.connect(opts)` creates a session and opens the socket. `close()` ends it. The instance also exposes the wire-level calls (`lease`, `runCommand`, `readFile`, `deleteLease`, `listCapabilities`, `limits` (tenant caps next to current consumption), `tenantUsage`, evidence and dashboard streams, ...).
- `Sandbox.create(client, opts)` leases a sandbox. `Sandbox.fork`, `Sandbox.get` (by name) and `Sandbox.list` return existing leases.
- On a `Sandbox`: `runCommand` (pass `detached: true` for a `Command` you can `wait()`, `kill()` or feed stdin), `writeFiles`, `readFile`, `readFileToBuffer`, `fs` (`ls`, `stat`, `mkdir`, `rm`, `mv`, `cp`, `chmod`, ...; plus `checkpoint`, `changes` and `watch`, an async iterator of create/modify/delete events, which need a provider with change tracking or file watch support; plus `uploadFile`, `downloadFile`, `uploadDir` and `downloadDir`, which move large files and whole trees in chunks, resume from the bytes already present and verify a SHA-256, and need a provider with directory transfer support), `stop`, `delete`, `extendTimeout`, `usage` (counters and, where the provider reports them, cpu and memory gauges), `updateTags`, `updateNetworkPolicy`, `domain`, `createUser`, `createLspServer`.

See [llms.txt](llms.txt) for a compact reference, and the [Hono](docs/hono.md) and [Next.js](docs/nextjs.md) guides for backend use.

Errors from the gateway are thrown as typed errors from `errors.ts`.

## Development

```sh
pnpm typecheck
pnpm test
pnpm test:coverage
pnpm build
```

The test suite uses the in-memory client and local transport doubles. It does not establish live provider support. The test command also builds the package and imports its emitted ESM entry point under Node.

`check:doc-snippets` type-checks this README and the managed-server lifecycle guide against the public SDK sources. The Hono and Next.js guides are integration templates that require their respective frameworks and application context.

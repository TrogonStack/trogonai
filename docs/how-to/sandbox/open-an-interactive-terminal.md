# Open an interactive terminal

Open a pseudo terminal in a running sandbox, type into it, resize it and read its output live. Use it for shells, REPLs, editors and anything else that checks `isatty`. For one-off commands use `run_command`; for line-oriented input to a detached command use interactive stdin.

## How it works

A terminal is a long-lived, bidirectional session, so it does not fit the request and reply shape of `run_command`. It uses two paths:

- Control (open, input, resize, close) is one request and reply verb, `terminal`, on `sandbox.lease.terminal.<lease_id>`. The broker authenticates every frame against the lease's tenant, checks that the provider advertises `supports_terminal` (otherwise it replies `Unsupported`), then forwards the frame to the provider's terminal subject for that lease.
- Output is published by the worker on core NATS at `sandbox.terminal.out.<terminal_id>` as `TerminalOutput` frames: `Data { seq, data }` (base64 of raw PTY bytes, merged stdout and stderr, at most 16 KiB per frame), `Resync { from_seq }` after a reattach that lost output, and one final `Exit { seq, exit_code }`.

Output is on core NATS, not JetStream, on purpose. A terminal carries keystroke echo, including typed passwords, and a durable stream would persist it. The cost is that output is live and single writer: a reader that is not subscribed misses frames, and a slow reader loses frames. `seq` increases by one per frame, so a gap tells the reader it lost output. Read durable command logs with `read_logs` instead when you need history.

Every worker that serves a PTY keeps a small replay ring per terminal (`TerminalReplayRing` in `trogon-sandbox-protocol`): the last few KiB of output, 8 KiB by default, held in memory only, never persisted, and dropped with the terminal. Set `SANDBOX_TERMINAL_REPLAY_BYTES` on the worker to change the size (at most 1 MiB; `0` turns replay off). The subprocess, Docker, Kubernetes and Firecracker workers feed it.

To reattach, open the same terminal id again on the same lease and set `resume_from` on the `Open` to the highest `seq` you already saw (leave it out when you have seen nothing). The worker answers `Opened` without starting a second program and then publishes, ahead of any live frame:

- the ring's frames above `resume_from`, with their original `seq`, when the ring still covers everything you missed; or
- one `Resync { from_seq }` frame first, then the ring's frames, when output you have not seen was already dropped from the ring (or the ring is off). `Resync` means every byte before `from_seq` is gone: discard your screen state and treat `from_seq - 1` as your high-water mark.

Skip any frame at or below the highest `seq` you already saw. Without `resume_from` the worker replays the whole ring and never sends `Resync`. The TypeScript client surfaces the frame as `{ type: 'resync', fromSeq }` in the terminal iterator, and `trogon-sandbox-mcp` counts it as lost output.

## Limits

| Limit | Value |
| --- | --- |
| Terminal width or height | 1 to 1000 |
| Input per `input` call | 64 KiB |
| Output per frame | 16 KiB |
| Terminals per lease | 8 (`TooManyTerminals`) |
| Terminals per gateway socket | 32 |
| Terminal id length | 1 to 64 |

## Step 1: Open

Through the gateway, call `sandbox_terminal_open` with `lease_id`, `cols`, `rows` and optionally `cmd`, `env` and `cwd`. An empty `cmd` runs the sandbox's default shell (`TERM` defaults to `xterm-256color`).

```ts
const term = await sandbox.terminal({ cols: 120, rows: 40 });
```

`cols` and `rows` default to 80 and 24 in the TypeScript client.

The gateway, not the client, picks the terminal id and subscribes to its output before it sends the open. Two things follow: the first prompt is never missed, and a client cannot name another tenant's terminal id to listen in on it. Treat the id as a bearer capability and never log it.

Direct NATS clients use `terminal(lease_id, terminal_id, op)` with a `TerminalOp::Open` and `terminal_output(&terminal_id)` from `rsworkspace/crates/sandbox/trogon-sandbox-client`. Subscribe to the output before you open.

From an MCP client (an agent), `trogon-sandbox-mcp` offers `sandbox_terminal_open`, `sandbox_terminal_write`, `sandbox_terminal_read`, `sandbox_terminal_resize` and `sandbox_terminal_close`. An agent cannot hold a live stream, so the MCP server subscribes for it and buffers up to 256 KiB per terminal. `sandbox_terminal_read` drains that buffer, waiting up to `wait_ms` (at most 20 seconds) when it is empty, and reports `dropped_bytes` and `missed_frames` when output was lost. `exited` turns true once the terminal ended and all of its output was read. The server refuses `sandbox_terminal_open` when it runs with a command policy (`--deny-command` or `--allow-command`), since typed input cannot be checked against it.

## Step 2: Type, resize, read

```ts no-check
await term.write('ls -la\n');
await term.resize(160, 50);
for await (const chunk of term) {
  if (chunk.type === 'data') process.stdout.write(chunk.bytes);
  else break; // exit
}
```

`write` carries raw bytes, so control sequences (`\x03` for Ctrl-C) work as on a real terminal. Calling input, resize or close for an id the socket did not open fails with `terminal_failed` and kind `not_found`.

## Step 3: Close

`close` ends the terminal and is safe to repeat. The worker emits exactly one `Exit` frame whichever way the terminal ends (the shell exits, `close`, the lease is released). When a gateway socket drops, the gateway closes every terminal that socket opened. Releasing the lease also ends its terminals.

## Errors

| Reply | Meaning |
| --- | --- |
| `Unsupported` (`unsupported`) | The provider has no PTY. Pick another provider. |
| `Failed(NotFound)` | No such terminal on this lease. |
| `Failed(IdInUse)` | The id belongs to another lease. |
| `Failed(TooManyTerminals)` | The lease is at its cap. |
| `Failed(InvalidRequest)` | Bad size, id or input; not retryable as sent. |
| `Failed(SpawnFailed)` | The program could not start. |
| `Failed(Exited)` | The terminal has already exited. |
| `Denied` | The lease is unknown, expired or belongs to another tenant. |

## Provider behavior

- Docker: a TTY exec inside the lease's container, started through a small `sh` wrapper that records the shell's pid so `close` can send it SIGHUP with a second exec. Resize uses the exec resize call, and the exit code comes from the exec inspect.
- Memory worker: a scripted echo terminal for tests.
- Subprocess worker: each terminal is a separate launch of the lease's isolation backend (same binds, network and limits) with a fresh pseudo terminal as its stdin, stdout and stderr and as its controlling terminal. The worker holds the master side; `close` or the lease's release sends SIGHUP to the terminal's process group and SIGKILL after a grace period. The profile advertises terminals on Linux and macOS and not on Windows.
- KubeVirt worker: each terminal is a channel with its own pty on one SSH session from the worker to the guest's `sshd`. On a lease's first terminal the worker generates an ephemeral ed25519 key held in memory, installs its public half in the guest's `authorized_keys` through the guest-exec path, and pins the first host key the guest presents for the lease's lifetime. The session and key are dropped when the lease is released. The guest image must run `sshd`.
- E2B worker: each terminal is an `envd` PTY process started through `process.Process/Start`. Input is `SendInput`, resize is `Update`, and close is a SIGKILL through `SendSignal`.
- The Kubernetes and Firecracker workers also advertise `supports_terminal`.
- Every other worker declares `supports_terminal` as unsupported.

## Operational notes

- Idle timeout: each control frame (open, input, resize, close) counts as lease activity and extends the idle timer. While output flows the worker also publishes a throttled activity signal (at most one every 5 seconds per terminal) that the broker counts toward the idle timeout, so a terminal that only prints keeps its lease from idling out. See [Pause or release idle sandboxes](./pause-idle-sandboxes.md).
- Subprocess limits: each terminal is its own sandbox launch with its own copy of the lease's CPU and memory limits, so a lease with open terminals can use more than one launch's budget. A command left running in the background keeps the terminal open until `close`.
- NATS permissions: `sandbox.terminal.out.>` carries terminal bytes for every tenant. Only workers should publish there and only the gateway (or a trusted service) should subscribe. Do not hand end users raw NATS credentials that can subscribe to it. See [Client NATS permissions](../../reference/sandbox/client-nats-permissions.md).

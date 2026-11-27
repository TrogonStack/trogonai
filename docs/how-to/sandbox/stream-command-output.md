# Stream live stdout and stderr from a running command

This guide explains how to receive a running command's output incrementally instead of waiting for it to finish. Use it when you build interactive tooling such as follow-mode logs, progress bars or long-running build output.

## How it works

By default, `run_command` buffers stdout and stderr and returns them when the command finishes, or when you poll a detached execution. Setting `stream_output` to `Enabled` in `CommandOptions` additionally publishes each chunk of output as a `CommandOutputChunk` event on the lease's evidence stream, at the subject:

```text
sandbox.evidence.{lease_id}.command.{execution_id}
```

Buffering still happens regardless of `stream_output`. Streaming is additive: a caller that never subscribes to the evidence stream sees the same behavior as with streaming disabled.

## Start the command with streaming enabled

```rust
let options = CommandOptions {
    timeout_seconds: 30,
    stream_output: StreamedOutput::Enabled,
    ..Default::default()
};
let execution_id = client
    .run_command_detached(
        &lease_id,
        vec!["bash".into(), "-c".into(), "for i in $(seq 1 20); do echo tick $i; sleep 1; done".into()],
        options,
        StdinMode::Closed,
        CommandDeadline::Timeout,
    )
    .await?;
```

Use a detached execution when you intend to stream, because you need the `execution_id` to pick the right frames.

## Subscribe to the evidence stream and filter by execution

```rust
use trogon_sandbox_protocol::subjects::evidence_command_suffix;

let wanted_suffix = evidence_command_suffix(&execution_id);
let mut evidence = client.evidence_stream(&lease_id).await?;

while let Some(frame) = evidence.next().await {
    let frame = frame?;
    if frame.subject_suffix != wanted_suffix {
        continue; // evidence from other commands or events on the same lease
    }
    let chunk: CommandOutputChunk = serde_json::from_slice(&frame.payload)?;
    print!("{}", String::from_utf8_lossy(&chunk.data));
}
```

The evidence stream carries every evidence frame published for the lease, not only command output, so compare `subject_suffix` against `evidence_command_suffix(&execution_id)` to isolate one command. `CommandOutputChunk.seq` increases monotonically per stream, so a consumer can detect gaps or reordering. `data` is raw bytes. It is not guaranteed to be valid UTF-8 or to hold one line per chunk.

## Consume the stream from TypeScript

```typescript
const command = await sandbox.runCommand({
  cmd: 'npm',
  args: ['run', 'build'],
  detached: true,
  streamOutput: true,
});

for await (const { stream, data } of command.stream()) {
  process.stdout.write(new TextDecoder().decode(data));
}
```

`Command.stream()` filters the sandbox's evidence stream down to this command's `command.<execution_id>` chunks and decodes each one. It throws if the command was not started with `streamOutput: true`. `Command.logs()` remains the buffered alternative for commands that were not started with streaming.

## Send input to a running command

A detached command can keep its stdin open. Only providers whose profile sets `supports_interactive_stdin` accept it: the docker, k8s, firecracker and substrate workers. Other providers deny the command with `interactive stdin is not supported by this provider`.

### Start the command with interactive stdin

```rust
let options = CommandOptions {
    timeout_seconds: 300,
    stream_output: StreamedOutput::Enabled,
    output_retention: OutputRetention::StreamOnly,
    ..Default::default()
};
let execution_id = client
    .run_command_detached(
        &lease_id,
        vec!["cat".into()],
        options,
        StdinMode::Interactive,
        CommandDeadline::Timeout,
    )
    .await?;
```

`StdinMode::Interactive` requires a detached command. Pass `OutputRetention::StreamOnly` for a long-lived process: the worker then keeps no stdout for polling and only a bounded stderr tail (64 KiB), so memory does not grow. Pair it with streaming to receive output on the evidence stream. Leave it at `Buffer` when you want the full output when polling at the end.

### Write with a sequence number

```rust
let response = client
    .write_stdin(
        &lease_id,
        &execution_id,
        StdinSeq(0),
        StdinChunk::new(b"hello\n".to_vec())?,
        StdinEof::Keep,
    )
    .await?;
```

Number writes from zero and increase `seq` by one per chunk. A chunk carries at most `StdinChunk::MAX_LEN` bytes (64 KiB), and a write with no data and no end-of-input marker is rejected. The worker applies each `seq` once and in order, so a retried write with the same `seq` is safe: the reply is `Duplicate` instead of a second delivery.

### Close stdin

Send a final write with `StdinEof::Close`, with or without data, at the next `seq`. Further writes reply `StdinClosed`.

### Handle the reply

Every ordered reply carries `next_seq`, the next `seq` the worker will apply.

| Reply | What to do |
|---|---|
| `Written` | Continue from `next_seq`. |
| `Duplicate` | The chunk was already applied. Continue from `next_seq`. |
| `Buffered` | The chunk arrived early and is held until the gap fills. Keep sending. |
| `Backpressure` | The process is not draining its stdin. Wait and resend from `next_seq`. |
| `OutOfWindow` | The chunk is too far ahead to hold. Resend from `next_seq`. |
| `StdinClosed`, `AlreadyCompleted`, `NotFound` | Stop writing. |
| `StdinNotInteractive`, `Unsupported`, `Denied` | The command or provider cannot take input. Fix the request. |

The worker holds at most 32 chunks (`StdinSequencer::REORDER_WINDOW_CHUNKS`) or 1 MiB (`REORDER_WINDOW_BYTES`) of out-of-order writes, both defined in `rsworkspace/crates/sandbox/trogon-sandbox-protocol/src/stdin_sequencer.rs`. Sending in order and waiting for each reply stays well inside that window. After a transport timeout, resend the same `seq` rather than guessing whether it landed.

Read the result by polling the execution, which returns `Running` until the process exits and then `Completed`. From TypeScript, use `command.writeStdin(data, { eof })` and `command.closeStdin()`.

### From the MCP server

`sandbox_run_command` accepts `stdin` (`closed` or `interactive`) and `output_retention` (`buffer` or `stream_only`) alongside `detached`. `sandbox_write_stdin` sends a chunk with its `seq` and `eof` and returns the same replies.

## Stop and restart a running command

Only providers whose profile sets `supports_graceful_stop` and `supports_command_restart` take these: the docker, k8s and in-memory workers. Others reply `Unsupported`.

```rust
let stopped = client.stop_command(&lease_id, &execution_id, Some(5)).await?;
// StopCommandResponse::Stopped { signal: Term | Kill }, AlreadyCompleted, NotFound, Unsupported

let restarted = client.restart_command(&lease_id, &execution_id, None).await?;
// RestartCommandResponse::Restarted { execution_id }, NotFound, Unsupported
```

`stop_command` sends SIGTERM, waits `grace_seconds` (default 10, capped at 300), then sends SIGKILL. `kill_command` stays the way to send one signal without a wait. `restart_command` stops the old execution the same way and runs the original request again under a new execution id, so poll and stream the new id. In TypeScript, `command.stop({ graceMs })` and `command.restart()` do the same and throw an error with code `unsupported` when the provider cannot.

## Read durable logs and resume from a cursor

The evidence stream above is core NATS: a subscriber that was not connected sees nothing. The broker therefore also keeps every streamed chunk in a JetStream stream named `sandbox_command_logs` (subjects `sandbox.evidence.*.command.*`), and `read_logs` pages it.

```rust
let mut after = 0;
loop {
    let ReadLogsResponse::Logs { chunks, next_seq, has_more, .. } =
        client.read_logs(&lease_id, &execution_id, after, None).await?
    else {
        break; // Unsupported: this broker keeps no durable logs
    };
    for chunk in chunks {
        std::io::stdout().write_all(&chunk.data)?;
    }
    after = next_seq;
    if !has_more.has_more() {
        break;
    }
}
```

The cursor is `log_seq`, the JetStream stream sequence, not the worker's `CommandOutputChunk.seq`: the docker worker numbers stdout and stderr separately, so `seq` is not unique per execution. Start at `0` for the oldest retained chunk and pass the previous page's `next_seq` to resume, including after a dropped connection or a restarted client.

`max_bytes` defaults to 64 KiB and is capped at 512 KiB. The `has_more` field means the page stopped at `max_bytes`. The `truncated` field means retention already dropped the start of this execution's output, so the first chunk is not the first byte the command wrote. A page is read once. In TypeScript use `command.logPage({ after, maxBytes })`. To follow a running command, see the next section.

`read_logs` requires an active lease, authenticated against its tenant like every other lease verb. Once the lease is released the verb replies denied, even though the stream keeps the chunks until retention drops them.

## Follow a running command from a cursor

Following replays the retained output after a cursor, then delivers new chunks as they are produced, and ends once the command exits. Because the cursor is the durable `log_seq`, a dropped connection resumes with no gaps and no repeats.

`read_logs_waiting` turns `read_logs` into a long poll: when nothing is retained after the cursor, the broker holds the request until a chunk lands or the wait passes (capped at 20 seconds), then replies as usual. An empty `Logs` reply with no continuation just means nothing arrived in time. Authorization runs on every request, so a tenant can follow only its own leases.

```rust
let mut events = client.follow_logs(&lease_id, execution_id, last_log_seq);
while let Some(event) = events.next().await {
    match event? {
        LogFollowEvent::Chunk(chunk) => {
            std::io::stdout().write_all(&chunk.data)?;
            last_log_seq = chunk.log_seq;
        }
        LogFollowEvent::End { next_seq, truncated } => break, // the command exited
        LogFollowEvent::Unsupported => break, // no durable logs on this broker
        LogFollowEvent::Reconnecting { attempt } => { /* a read failed; retrying */ }
        LogFollowEvent::ResyncRequired { from_cursor } => { /* output was dropped */ }
    }
}
```

A broker timeout or transport failure is retried in place, up to five times in a row, with a short growing backoff. Each retry is announced as `Reconnecting` and resumes from the same cursor, so nothing is lost or repeated. When the retries run out, or on any other error, an `Err` item ends the stream. Start `follow_logs` again with the last `log_seq` you saw. The end marker follows the exit by up to a few seconds when the command is silent, because exit is detected by polling the execution.

`ResyncRequired { from_cursor }` means retention dropped output the follower had not read yet: a hole between `from_cursor` (the last cursor delivered, `0` when you read from the start) and the next chunk. The stream keeps going from the oldest retained chunk. Treat the output as incomplete; it never appears as a silent gap. It is raised when a per-stream `seq` skips, and when you follow from cursor `0` and the start of the output is already gone.

Through the websocket gateway, `sandbox_follow_logs` takes the same payload as `sandbox_read_logs` (`after_seq` is the cursor), replies `{ following: true, stream_id }` and then sends event frames on the stream id `logs:<lease_id>:<execution_id>`: `chunk` (with `log_seq`, `stream`, `seq`, `data`), `reconnecting` (`attempt`) and `resync_required` (`from_cursor`) as informational events, then one of `end`, `unsupported` or `error`.

In TypeScript:

```typescript no-check
let cursor = 0;
for await (const chunk of command.logs({ follow: true, after: cursor })) {
  process.stdout.write(chunk.data);
  cursor = chunk.cursor;
}
```

The iterator ends when the command exits. It ignores `reconnecting` and throws a `resync_required` error when output was dropped, so a consumer of plain chunks never reads a stream with a silent hole. If the connection drops it throws a `TransportError`; call `command.logs({ follow: true, after: cursor })` on a new connection to continue. It throws `unsupported` when the broker keeps no durable logs. Follow one command per connection at a time.

### From the MCP server

`sandbox_run_command` accepts `stream_progress: true` (with `detached` false). When the call carries a `progressToken`, each output chunk is sent as an MCP progress notification whose `message` is the chunk text (stderr is prefixed with `[stderr] `) and whose `progress` counts chunks. The tool result is the same completed result a normal call returns. Without a progress token, or when the broker keeps no durable logs, no notifications are sent and the call behaves as usual.

### Retention

The stream is bounded, oldest first, by these broker settings:

| Flag | Environment variable | Default |
|---|---|---|
| `--command-log-max-bytes` | `SANDBOX_COMMAND_LOG_MAX_BYTES` | 1 GiB across all executions |
| `--command-log-max-age-secs` | `SANDBOX_COMMAND_LOG_MAX_AGE_SECS` | 24 hours |
| `--command-log-max-chunks-per-execution` | `SANDBOX_COMMAND_LOG_MAX_CHUNKS_PER_EXECUTION` | 16384 chunks per execution |
| `--command-exit-max-age-secs` | `SANDBOX_COMMAND_EXIT_MAX_AGE_SECS` | 7 days (never below the output age) |

Exit status outlives output. Besides the output stream, the broker runs a second stream, `sandbox_command_exits`, that sources only the exit markers and keeps one per command for `--command-exit-max-age-secs`. A reader that arrives after the output aged out still gets the exit report (and the follower ends with `End`) instead of waiting on a marker that was dropped with the chunks. If the second stream cannot be created the broker logs a warning and exit status ages out with the output.

The stream uses file storage and discards the oldest messages when a bound is hit. If the broker cannot create the stream at startup (for example an operator already owns a stream that overlaps `sandbox.evidence.*.command.*`), it logs a warning and runs without durable logs. `read_logs` then replies `Unsupported` and live streaming is unaffected. The `truncated` field can read false when the head of one stream (stdout or stderr) was dropped while the other still holds its first chunk.

## When to use streaming and when to buffer

Use streaming when you need to show output as it happens, for example a follow-mode log viewer or a progress indicator for a long build. Leave it disabled (the default) for short commands where you only care about the final result. Buffering alone is cheaper and simpler for that case, and you get the full output when you poll either way.

## See also

- [Observability](./observability.md) for the broader evidence-stream and tracing plumbing this feature reuses.

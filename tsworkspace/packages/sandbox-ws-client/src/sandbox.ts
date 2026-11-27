import { z } from 'zod';

import { SandboxCapabilities } from './capabilities.ts';
import type { SandboxFeature } from './capabilities.ts';
import type { SandboxClientApi } from './client-api.ts';
import type { FileChangeSet, FileWatchItem } from './client.ts';
import { cancelOwnedCommand, cancellationRunResponse, cancellationTimeout } from './command-cancellation.ts';
import { cancelPendingCommandLaunch, retainCommandLaunchAuthority } from './command-launch.ts';
import {
  CommandCancellationError,
  CommandFailedError,
  PreviewNotReadyError,
  ServerExitedError,
  SandboxNotFoundError,
  SandboxWsError,
  StdinError,
  TransportError,
  TimeoutError,
} from './errors.ts';
import type { CommandCleanupReceipt, RestoreCleanupReceipt } from './errors.ts';
import { SandboxGit } from './git.ts';
import { executionId, stdinSeq } from './ids.ts';
import type { Digest, LeaseId, TenantId } from './ids.ts';
import { LSP_PRESETS } from './lsp/presets.ts';
import type { LspPresetName } from './lsp/presets.ts';
import { LspServer } from './lsp/server.ts';
import type { LspServerOptions, LspServerSpec } from './lsp/server.ts';
import { allowListPolicy } from './network.ts';
import type { NetworkAllowList } from './network.ts';
import { cancelCreateOperation, operationCapabilitiesSchema, parseOperationResponse } from './operations.ts';
import { OWNER_TAG_KEY, OWNER_TAG_VALUE, ownershipFilter, withOwnershipTag } from './ownership.ts';
import {
  defaultProcessName,
  parseProcessInput,
  parseProcessReady,
  processNameSchema,
  processStopOptionsSchema,
  startProcessSchema,
} from './process-state.ts';
import type { StartProcessParams } from './process-state.ts';
import {
  parseServerClientState,
  parseServerProcessInventory,
  requireServerEnvironment,
  snapshotClientState,
  snapshotExecutionContinuity,
} from './server-state.ts';
import type {
  RestorableSnapshot,
  SandboxNativeSnapshotReceipt,
  SandboxSnapshotReceipt,
  ServerClientState,
  ServerConnectionOptions,
  ServerRestartEntry,
  SnapshotExecutionContinuity,
} from './server-state.ts';
import { shellTemplate } from './shell.ts';
import {
  fileEditInputSchema,
  fileEditResultSchema,
  fileListInputSchema,
  fileListingSchema,
  fileSearchInputSchema,
  fileSearchResultSchema,
  parseFileInput,
  parseFileOutput,
} from './structured-files.ts';
import type { EditFileOptions, ListFilesOptions, SearchFilesOptions } from './structured-files.ts';
import type { DirFile, DownloadedDir, TransferReport } from './transfer.ts';
import { SandboxUser } from './user.ts';
import type {
  CommandDeadline,
  CommandLaunchOrigin,
  CapabilityRequirements,
  CapabilityRequirementsInput,
  CommandOutputChunk,
  DriveRecord,
  ProviderVolumeRef,
  LeaseByNameRow,
  LeaseGranted,
  LeaseListRow,
  LeaseRequest,
  LeaseSnapshotReceipt,
  NativeSnapshotReceipt,
  NativeSnapshotRef,
  LeaseStatus,
  SuspendLevel,
  SuspendRequestLevel,
  NetworkPolicy,
  OciArtifactRef,
  OutputRetention,
  ProcessSpec,
  ProcessInfo,
  ProviderKind,
  ReapLeasesResponse,
  RunCommandResponse,
  SandboxSpec,
  SandboxTags,
  SecretBinding,
  StdinMode,
  StopCommandResponse,
  UnixSignal,
  WriteStdinResponse,
} from './wire.ts';
import {
  capabilityRequirementsSchema,
  MAX_READY_PORTS,
  MAX_STDIN_CHUNK_BYTES,
  MAX_TERMINAL_INPUT_BYTES,
  MAX_WAIT_SANDBOX_READY_SECONDS,
  wireSystemTimeToDate,
} from './wire.ts';
import type { IdleAction, SshAccess, TerminalOutputEvent } from './wire.ts';

/**
 * Convenience input for network policy. `'allow-all'` and
 * `'deny-all'` map onto the wire's `allow_all`/`deny_all` string variants;
 * `{ allow: ['registry.npmjs.org'] }` builds a deny-by-default policy that
 * permits only those hosts (see `allowListPolicy`); anything else (the wire's
 * `{ user_defined: {...} }` shape) passes through unchanged.
 */
export type NetworkPolicyInput = 'allow-all' | 'deny-all' | NetworkAllowList | NetworkPolicy;

function toWireNetworkPolicy(policy: NetworkPolicyInput): NetworkPolicy {
  if (policy === 'allow-all') {
    return 'allow_all';
  }
  if (policy === 'deny-all') {
    return 'deny_all';
  }
  if (typeof policy === 'object' && 'allow' in policy) {
    return allowListPolicy(policy.allow);
  }
  return policy;
}

/** One chunk of `Command.logs({ follow: true })`; `cursor` is the value to resume after. */
export interface FollowedChunk {
  stream: 'stdout' | 'stderr';
  data: string;
  bytes: Uint8Array;
  cursor: number;
}

function decodeBytes(bytes: number[]): Uint8Array {
  return Uint8Array.from(bytes);
}

/**
 * Environment merged under the caller's `env` on every non-interactive run so
 * a tool that would stop to ask a question fails or proceeds instead of
 * hanging. Cosmetic variables such as `TERM=dumb` and `NO_COLOR=1` are left
 * out because they change output rather than prevent stalls.
 */
export const NON_INTERACTIVE_ENV: Readonly<Record<string, string>> = Object.freeze({
  CI: '1',
  DEBIAN_FRONTEND: 'noninteractive',
  GIT_TERMINAL_PROMPT: '0',
  PAGER: 'cat',
});

/**
 * The env sent for a run: the defaults under the caller's `env`, so the caller
 * wins per variable. Defaults are skipped when the caller opts out with
 * `nonInteractive: false`, and for `stdin: 'interactive'`, where prompts are
 * answered through `Command.writeStdin`.
 */
export function withNonInteractiveEnv(
  env: Record<string, string> | undefined,
  opts: { nonInteractive?: boolean; stdin?: StdinMode } = {},
): Record<string, string> | undefined {
  if (opts.nonInteractive === false || opts.stdin === 'interactive') {
    return env;
  }
  return { ...NON_INTERACTIVE_ENV, ...env };
}

/** Cpu and memory figures; each is undefined when the provider cannot report it. */
export interface SandboxUsageGauges {
  cpuNanosTotal?: number;
  memoryBytes?: number;
  memoryPeakBytes?: number;
  memoryLimitBytes?: number;
}

/** What `Sandbox.usage()` returns. */
export interface SandboxUsage {
  status: LeaseStatus;
  createdAt: Date;
  /** Set once the sandbox has ended; the figures are then final. */
  endedAt?: Date;
  lastActivityAt: Date;
  commandsRun: number;
  commandOutputBytes: number;
  fileBytesIn: number;
  fileBytesOut: number;
  /** Seconds since creation, excluding time spent paused. */
  runningSeconds: number;
  pausedSeconds: number;
  /** Time spent suspended, apart from running and paused time. */
  suspendedSeconds: number;
  /** Bytes the suspend artifact takes, when the provider reports it. */
  suspendArtifactBytes?: number;
  gauges?: SandboxUsageGauges;
}

export interface RunCommandParams {
  cmd: string;
  args?: string[];
  cwd?: string;
  /** Overrides `NON_INTERACTIVE_ENV` per variable. */
  env?: Record<string, string>;
  /**
   * Merge `NON_INTERACTIVE_ENV` under `env`. Defaults to true; pass `false` to
   * send exactly `env`. Never applied with `stdin: 'interactive'`.
   */
  nonInteractive?: boolean;
  detached?: boolean;
  streamOutput?: boolean;
  user?: string;
  /** `interactive` requires `detached: true`; feed it with `Command.writeStdin`. */
  stdin?: StdinMode;
  outputRetention?: OutputRetention;
  timeoutSeconds?: number;
  /** `lease` removes the per-command timeout and requires `detached: true`. */
  deadline?: CommandDeadline;
  /** Per-stream retention cap in bytes; the broker clamps it to its ceiling. */
  maxOutputBytes?: number;
  /** Names a detached command and optionally attaches a readiness probe. */
  process?: ProcessSpec;
  /** Exact literals replaced with `[REDACTED]` in the output. Encoded copies are not caught. */
  redactValues?: string[];
  /**
   * Requires `command_cancellation`. The additive `command_launch_cancellation`
   * capability retains authority through lost start replies and permits named
   * requests while preserving reused or unknown work. Foreground requests use
   * an early detached identity and wait for completion. Inspect `cleanupPending`
   * and the retained `launchKey` when cancellation needs reconciliation.
   */
  signal?: AbortSignal;
  /** Abort acknowledgement and cleanup budget, 1 to 60000 ms; defaults to 10000. */
  cancellationTimeoutMs?: number;
}

/**
 * `CommandFinished` is the result of an already-completed command. This
 * client has no push-based log stream: `logs()`
 * is only ever able to replay the two buffered chunks captured at
 * completion, not a live feed.
 */
export class CommandFinished {
  readonly cmdId: string;
  readonly sandboxId: LeaseId;
  readonly exitCode: number;
  private readonly stdoutBytes: Uint8Array;
  private readonly stderrBytes: Uint8Array;

  /** The command hit its deadline and was killed. */
  readonly timedOut: boolean;
  /** stdout or stderr exceeded the retention cap and its middle was dropped. */
  readonly truncated: boolean;
  /**
   * Wall-clock milliseconds the call took as seen by this client, so it
   * includes the round trip. Set for synchronous runs only; undefined for a
   * detached command collected with `Command.wait`, whose start time the
   * wire does not report.
   */
  readonly durationMs?: number;

  constructor(
    sandboxId: LeaseId,
    cmdId: string,
    exitCode: number,
    stdoutBytes: Uint8Array,
    stderrBytes: Uint8Array,
    flags: { timedOut?: boolean; truncated?: boolean; durationMs?: number } = {},
  ) {
    this.durationMs = flags.durationMs;
    this.timedOut = flags.timedOut ?? false;
    this.truncated = flags.truncated ?? false;
    this.sandboxId = sandboxId;
    this.cmdId = cmdId;
    this.exitCode = exitCode;
    this.stdoutBytes = stdoutBytes;
    this.stderrBytes = stderrBytes;
  }

  /** True when the command exited 0 and did not hit its deadline. */
  get ok(): boolean {
    return this.exitCode === 0 && !this.timedOut;
  }

  stdout(): string {
    return new TextDecoder().decode(this.stdoutBytes);
  }

  /** stdout as text, with one trailing newline removed. */
  text(): string {
    return this.stdout().replace(/\r?\n$/, '');
  }

  /** Non-empty lines of stdout, with `\r` removed. */
  lines(): string[] {
    return this.stdout()
      .split('\n')
      .map((line) => line.replace(/\r$/, ''))
      .filter((line) => line.length > 0);
  }

  /** stdout parsed as JSON. Throws `SyntaxError` when it is not valid JSON. */
  json<T = unknown>(): T {
    return JSON.parse(this.stdout()) as T;
  }

  /** Returns this result when `ok`; otherwise throws `CommandFailedError` carrying the exit code and stderr. */
  throwIfFailed(): this {
    if (!this.ok) {
      throw new CommandFailedError(this.exitCode, this.stderr(), { timedOut: this.timedOut, leaseId: this.sandboxId });
    }
    return this;
  }

  stderr(): string {
    return new TextDecoder().decode(this.stderrBytes);
  }

  output(): { exitCode: number; stdout: string; stderr: string } {
    return { exitCode: this.exitCode, stdout: this.stdout(), stderr: this.stderr() };
  }

  // Post-hoc only: yields the two buffered chunks captured at completion,
  // not a live stream. See the class-level doc comment.
  async *logs(): AsyncGenerator<{ stream: 'stdout' | 'stderr'; data: string }, void, void> {
    yield { stream: 'stdout', data: this.stdout() };
    yield { stream: 'stderr', data: this.stderr() };
  }
}

export interface CommandWaitOptions {
  pollIntervalMs?: number;
  timeoutMs?: number;
}

export interface CommandCancelOptions {
  /** 1 to 60000 ms; defaults to 10000 or the signal-bound command's budget. */
  timeoutMs?: number;
}

interface CommandCancellationContext {
  supported: boolean;
  signal?: AbortSignal;
  timeoutMs?: number;
  launchKey?: string;
  named?: boolean;
}

export interface WaitUntilReadyOptions {
  /** Sandbox-side ports that must accept connections. */
  ports?: number[];
  /** Defaults to 30 000; the broker never waits more than 300 000. */
  timeoutMs?: number;
}

export interface StartServerParams extends Omit<
  RunCommandParams,
  'detached' | 'process' | 'deadline' | 'stdin' | 'streamOutput' | 'timeoutSeconds'
> {
  /** Port the server listens on inside the sandbox. */
  port: number;
  /**
   * HTTP path that must answer before the server counts as ready, for example
   * `/healthz`. Probed from inside the sandbox with `curl` (or `wget`), so the
   * image needs one of them. Any status except 502, 503 and 504 counts as
   * answering. Without a path an accepted TCP connection is enough.
   */
  path?: string;
  /** Names the process so it can be found and stopped later. */
  name?: string;
  /** How long to wait for readiness. Defaults to 60 000. */
  timeoutMs?: number;
  /** Resolve a preview URL once ready. Defaults to true; the call fails if the provider cannot. */
  preview?: boolean;
  /**
   * Start the server again, with these same params, when `resume()` or `wake()`
   * finds its process gone. Defaults to true. A server you stopped with
   * `server.stop()` is not restarted.
   */
  restartOnResume?: boolean;
}

export interface StartedServer {
  /** The running process; it lives until the lease ends or you `kill` it. Replaced by `restart()`. */
  readonly command: Command;
  readonly name?: string;
  readonly port: number;
  /** Preview URL of the server root, absent when `preview` was false. Refreshed by `restart()`. */
  readonly url?: string;
  readonly expiresAt?: Date;
  /**
   * Stops the process if it still runs, starts it again with the original
   * params, waits for readiness and refreshes `command`, `url` and `expiresAt`.
   * Throws what `startServer` throws.
   */
  restart(): Promise<void>;
  /** Stops the process and forgets the server, so a later `resume()` leaves it down. */
  stop(): Promise<void>;
}

interface ServerSpec {
  run: Omit<StartServerParams, 'port' | 'path' | 'name' | 'preview' | 'timeoutMs' | 'restartOnResume'>;
  port: number;
  path: string | undefined;
  name: string | undefined;
  preview: boolean | undefined;
  budgetMs: number;
}

interface LaunchedServer {
  command: Command;
  url?: string;
  expiresAt?: Date;
}

const DEFAULT_START_SERVER_TIMEOUT_MS = 60_000;
/** One readiness request to the broker, so an early exit is noticed within about this long. */
const START_SERVER_SLICE_SECONDS = 1;
const START_SERVER_PAUSE_MS = 100;
const START_SERVER_LOG_TAIL_CHARS = 4096;
const START_SERVER_MAX_LOG_PAGES = 50;
const RESTORE_CLEANUP_TIMEOUT_MS = 10_000;
const RESTORE_CLEANUP_POLL_MS = 50;
/** Prints the HTTP status of `$1`; `000` when nothing answered. */
const HTTP_PROBE_SCRIPT =
  'if command -v curl >/dev/null 2>&1; then curl -s -o /dev/null -w "%{http_code}" --max-time 5 "$1"; ' +
  'elif command -v wget >/dev/null 2>&1; then wget -q -O /dev/null -T 5 "$1" && printf 200 || printf 000; ' +
  'else printf 000; fi';

const DEFAULT_READY_TIMEOUT_MS = 30_000;
/** Longest one request blocks on the broker, so a lost reply costs at most this. */
const READY_SLICE_MS = 20_000;
const DEFAULT_POLL_INTERVAL_MS = 200;
const MAX_POLL_INTERVAL_MS = 2000;
const STDIN_MAX_TRANSPORT_RETRIES = 3;
const STDIN_RETRY_DELAY_MS = 50;
const STDIN_MAX_STALLED_RESENDS = 3;

export interface StdinBackpressureOptions {
  initialDelayMs?: number;
  maxDelayMs?: number;
  /** Wall-clock time, from the first attempt on one chunk, before `stdin_backpressure` is raised. */
  budgetMs?: number;
  /** Clock in milliseconds, injectable for tests. */
  now?: () => number;
}

const DEFAULT_BACKPRESSURE: Required<StdinBackpressureOptions> = {
  initialDelayMs: 50,
  maxDelayMs: 1000,
  budgetMs: 30_000,
  now: () => Date.now(),
};

interface StdinChunk {
  seq: number;
  data: Uint8Array;
  eof: boolean;
}

/**
 * A command execution inside a sandbox. Detached `runCommand` calls resolve
 * to a live `Command`; non-detached calls resolve directly to a
 * `CommandFinished` (see `Command.fromRunCommandResponse`).
 */
export class Command {
  readonly cmdId: string;
  readonly sandboxId: LeaseId;
  private readonly streaming: boolean;
  private nextStdinSeq = 0;
  private unacked: StdinChunk[] = [];
  private stdinEofQueued = false;
  private stdinFailure: StdinError | undefined;
  private stdinTail: Promise<void> = Promise.resolve();
  private readonly backpressure: Required<StdinBackpressureOptions>;
  private cancellation: Promise<CommandFinished> | undefined;
  private abortFailure: Promise<never> | undefined;
  private removeAbortListener: (() => void) | undefined;
  private finished: CommandFinished | undefined;
  private abortTriggered = false;

  constructor(
    private readonly client: SandboxClientApi,
    sandboxId: LeaseId,
    cmdId: string,
    streaming = false,
    backpressure: StdinBackpressureOptions = {},
    private readonly cancellationContext: CommandCancellationContext = { supported: false },
    readonly origin: CommandLaunchOrigin = 'unknown',
  ) {
    this.sandboxId = sandboxId;
    this.cmdId = cmdId;
    this.streaming = streaming;
    this.backpressure = { ...DEFAULT_BACKPRESSURE, ...backpressure };
    const signal = cancellationContext.signal;
    if (signal !== undefined) {
      this.abortFailure = new Promise<never>((_resolve, reject) => {
        const onAbort = () => {
          this.abortTriggered = true;
          this.removeAbortListener?.();
          void this.cancel().then(
            () =>
              reject(
                new CommandCancellationError(this.sandboxId, this.cmdId, signal.reason, false, {
                  launchKey: cancellationContext.launchKey,
                }),
              ),
            reject,
          );
        };
        this.removeAbortListener = () => signal.removeEventListener('abort', onAbort);
        signal.addEventListener('abort', onAbort, { once: true });
        if (signal.aborted) onAbort();
      });
      void this.abortFailure.catch(() => {});
    }
  }

  static fromRunCommandResponse(
    client: SandboxClientApi,
    sandboxId: LeaseId,
    response: RunCommandResponse,
    streaming = false,
    durationMs?: number,
    cancellationContext?: CommandCancellationContext,
  ): Command | CommandFinished {
    response = cancellationRunResponse(response);
    if (response.outcome === 'started') {
      return new Command(client, sandboxId, response.execution_id, streaming, {}, cancellationContext, response.origin);
    }
    return new CommandFinished(
      sandboxId,
      '',
      response.exit_code,
      decodeBytes(response.stdout),
      decodeBytes(response.stderr),
      { timedOut: response.timed_out, truncated: response.truncated, durationMs },
    );
  }

  wait(opts?: CommandWaitOptions): Promise<CommandFinished> {
    if (this.finished !== undefined) return Promise.resolve(this.finished);
    if (this.abortTriggered) return this.abortFailure!;
    const waiting = this.waitForCompletion(opts);
    return this.abortFailure === undefined ? waiting : Promise.race([waiting, this.abortFailure]);
  }

  private async waitForCompletion(opts?: CommandWaitOptions): Promise<CommandFinished> {
    const deadline = opts?.timeoutMs !== undefined ? Date.now() + opts.timeoutMs : undefined;
    let interval = opts?.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
    for (;;) {
      if (this.abortTriggered) return this.abortFailure!;
      const response = await this.client.pollCommand(this.sandboxId, this.cmdId);
      if (this.abortTriggered) return this.abortFailure!;
      if (response.outcome === 'completed') {
        this.removeAbortListener?.();
        this.finished = new CommandFinished(
          this.sandboxId,
          this.cmdId,
          response.exit_code,
          decodeBytes(response.stdout),
          decodeBytes(response.stderr),
          { timedOut: response.timed_out, truncated: response.truncated },
        );
        return this.finished;
      }
      if (response.outcome === 'not_found') {
        throw new Error(`command ${this.cmdId} not found`);
      }
      if (deadline !== undefined && Date.now() >= deadline) {
        throw new Error(`timed out waiting for command ${this.cmdId} to finish`);
      }
      await sleep(interval);
      interval = Math.min(interval * 2, MAX_POLL_INTERVAL_MS);
    }
  }

  /**
   * Appends bytes to the command's interactive stdin. Writes are serialized
   * per command and split into chunks of at most 64 KiB, each with its own
   * gapless seq. Transport errors and timeouts are retried with the same seq;
   * `duplicate` counts as success and `buffered`/`out_of_window` resend from
   * the server's `next_seq`. If a write rejects for a transport reason its
   * unacknowledged chunks are kept and sent before the next write, since the
   * seq space has no gaps. `stdin_closed`, `already_completed`, `not_found`,
   * `unsupported` and `denied` are fatal for every later write.
   */
  writeStdin(data: Uint8Array | string, opts?: { eof?: boolean }): Promise<void> {
    const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
    const eof = opts?.eof ?? false;
    return this.enqueueStdin(() => {
      if (this.stdinFailure) {
        throw this.stdinFailure;
      }
      if (this.stdinEofQueued) {
        throw new StdinError('stdin_closed', `stdin of command ${this.cmdId} is closed`);
      }
      this.queueStdinChunks(bytes, eof);
      return this.flushStdin();
    });
  }

  /** Sends EOF on stdin. Safe to call more than once. */
  closeStdin(): Promise<void> {
    return this.enqueueStdin(() => {
      if (this.stdinFailure) {
        throw this.stdinFailure;
      }
      if (!this.stdinEofQueued) {
        this.queueStdinChunks(new Uint8Array(0), true);
      }
      return this.flushStdin();
    });
  }

  private enqueueStdin(op: () => Promise<void>): Promise<void> {
    const result = this.stdinTail.then(op);
    this.stdinTail = result.catch(() => undefined);
    return result;
  }

  private queueStdinChunks(bytes: Uint8Array, eof: boolean): void {
    if (bytes.length === 0 && !eof) {
      return;
    }
    let offset = 0;
    do {
      const end = Math.min(offset + MAX_STDIN_CHUNK_BYTES, bytes.length);
      const last = end >= bytes.length;
      this.unacked.push({ seq: this.nextStdinSeq++, data: bytes.subarray(offset, end), eof: eof && last });
      offset = end;
    } while (offset < bytes.length);
    if (eof) {
      this.stdinEofQueued = true;
    }
  }

  private async flushStdin(): Promise<void> {
    let stalled = 0;
    while (this.unacked.length > 0) {
      const chunk = this.unacked[0];
      const response = await this.sendStdinChunk(chunk);
      if (response.outcome === 'already_completed' && chunk.eof) {
        this.unacked = [];
        return;
      }
      if (response.outcome === 'stdin_closed' || !('next_seq' in response)) {
        this.stdinFailure = new StdinError(
          response.outcome,
          `stdin write to command ${this.cmdId} failed: ${response.outcome}`,
        );
        this.unacked = [];
        throw this.stdinFailure;
      }
      const serverNext = response.next_seq;
      if (response.outcome === 'written' || response.outcome === 'duplicate' || serverNext > chunk.seq) {
        this.unacked = this.unacked.filter((c) => c.seq > chunk.seq && c.seq >= serverNext);
        stalled = 0;
        continue;
      }
      if (serverNext < chunk.seq) {
        throw new StdinError(
          'stdin_resend_unavailable',
          `server expects stdin seq ${serverNext} of command ${this.cmdId}, which was already acknowledged`,
        );
      }
      stalled += 1;
      if (stalled > STDIN_MAX_STALLED_RESENDS) {
        throw new StdinError('stdin_stalled', `stdin of command ${this.cmdId} made no progress at seq ${serverNext}`);
      }
    }
  }

  private async sendStdinChunk(chunk: StdinChunk): Promise<Exclude<WriteStdinResponse, { outcome: 'backpressure' }>> {
    let transportAttempt = 0;
    let backoffMs = this.backpressure.initialDelayMs;
    const startedAt = this.backpressure.now();
    for (;;) {
      let response: WriteStdinResponse;
      try {
        response = await this.client.writeStdin(
          this.sandboxId,
          executionId(this.cmdId),
          stdinSeq(chunk.seq),
          chunk.data,
          chunk.eof,
        );
      } catch (err) {
        const retryable = err instanceof TransportError || err instanceof TimeoutError;
        if (!retryable || transportAttempt >= STDIN_MAX_TRANSPORT_RETRIES) {
          throw err;
        }
        transportAttempt += 1;
        await sleep(STDIN_RETRY_DELAY_MS * transportAttempt);
        continue;
      }
      if (response.outcome !== 'backpressure') {
        return response;
      }
      const elapsedMs = this.backpressure.now() - startedAt;
      if (elapsedMs >= this.backpressure.budgetMs) {
        throw new StdinError(
          'stdin_backpressure',
          `stdin of command ${this.cmdId} stayed backpressured at seq ${chunk.seq} for ${elapsedMs}ms`,
        );
      }
      await sleep(backoffMs);
      backoffMs = Math.min(backoffMs * 2, this.backpressure.maxDelayMs);
    }
  }

  async kill(signal: UnixSignal = 'term'): Promise<void> {
    await this.client.killCommand(this.sandboxId, this.cmdId, signal);
  }

  /** Confirms completion of owned work; reused or unknown named executions are preserved. */
  cancel(opts: CommandCancelOptions = {}): Promise<CommandFinished> {
    if (this.cancellation !== undefined) return this.cancellation;
    if (!this.cancellationContext.supported) {
      return Promise.reject(
        new SandboxWsError('unsupported', 'the provider does not support verified command cancellation'),
      );
    }
    if (this.cancellationContext.named && this.origin !== 'created') {
      const preserved = this.origin === 'reused';
      this.cancellation = Promise.reject(
        new CommandCancellationError(this.sandboxId, this.cmdId, this.cancellationContext.signal?.reason, !preserved, {
          launchKey: this.cancellationContext.launchKey,
          preserved,
        }),
      );
      void this.cancellation.catch(() => {});
      return this.cancellation;
    }
    let timeoutMs: number;
    try {
      timeoutMs = cancellationTimeout(opts.timeoutMs ?? this.cancellationContext.timeoutMs);
    } catch (error) {
      return Promise.reject(error);
    }
    this.cancellation = cancelOwnedCommand(
      this.client,
      this.sandboxId,
      this.cmdId,
      timeoutMs,
      this.cancellationContext.signal?.reason,
      this.cancellationContext.launchKey,
    ).then((output) => {
      this.removeAbortListener?.();
      return new CommandFinished(
        this.sandboxId,
        this.cmdId,
        output.exit_code,
        decodeBytes(output.stdout),
        decodeBytes(output.stderr),
        {
          timedOut: output.timed_out,
          truncated: output.truncated,
        },
      );
    });
    void this.cancellation.catch(() => {});
    return this.cancellation;
  }

  /**
   * Stops the command gracefully: SIGTERM, then SIGKILL once `graceMs`
   * (default 10 seconds, capped at 300 seconds) has passed. Rounded up to
   * whole seconds. Throws `unsupported` when the provider cannot stop
   * gracefully; use `kill()` there.
   */
  async stop(opts: { graceMs?: number } = {}): Promise<StopCommandResponse> {
    const graceSeconds = opts.graceMs === undefined ? undefined : Math.max(0, Math.ceil(opts.graceMs / 1000));
    const response = await this.client.stopCommand(this.sandboxId, this.cmdId, graceSeconds);
    if (response.outcome === 'unsupported') {
      throw new SandboxWsError('unsupported', 'this provider does not support graceful stop');
    }
    return response;
  }

  /**
   * Stops the command and runs it again with the same process spec. Resolves
   * a new `Command` with a new `cmdId`; this one is finished. Throws
   * `unsupported` when the provider cannot restart, and `not_found` when the
   * execution is unknown to the worker.
   */
  async restart(opts: { graceMs?: number } = {}): Promise<Command> {
    const graceSeconds = opts.graceMs === undefined ? undefined : Math.max(0, Math.ceil(opts.graceMs / 1000));
    const response = await this.client.restartCommand(this.sandboxId, this.cmdId, graceSeconds);
    if (response.outcome === 'unsupported') {
      throw new SandboxWsError('unsupported', 'this provider does not support command restart');
    }
    if (response.outcome === 'not_found') {
      throw new SandboxWsError('not_found', `command ${this.cmdId} not found`);
    }
    return new Command(this.client, this.sandboxId, response.execution_id, this.streaming, this.backpressure, {
      supported: this.cancellationContext.supported,
    });
  }

  /**
   * Reads one page of the command's durable output after the cursor `after`
   * (0 starts at the oldest retained chunk). Unlike `stream()`, this survives
   * a dropped connection: resume with the page's `nextSeq`. Throws
   * `unsupported` when the broker keeps no durable logs. `truncated` means
   * retention already dropped the start of the output. `exited` means the
   * command has exited, so once `hasMore` is false there is nothing left.
   */
  async logPage(opts: { after?: number; maxBytes?: number } = {}): Promise<{
    chunks: { logSeq: number; stream: 'stdout' | 'stderr'; data: Uint8Array }[];
    nextSeq: number;
    hasMore: boolean;
    truncated: boolean;
    exited: boolean;
  }> {
    const response = await this.client.readLogs(this.sandboxId, this.cmdId, {
      afterSeq: opts.after,
      maxBytes: opts.maxBytes,
    });
    if (response.outcome === 'unsupported') {
      throw new SandboxWsError('unsupported', 'this broker does not keep durable command logs');
    }
    return {
      chunks: response.chunks.map((c) => ({ logSeq: c.log_seq, stream: c.stream, data: Uint8Array.from(c.data) })),
      nextSeq: response.next_seq,
      hasMore: response.has_more,
      truncated: response.truncated,
      exited: response.exited ?? false,
    };
  }

  /**
   * Without options, `logs()` is post-hoc: it waits for completion, then
   * replays the two buffered streams once each.
   *
   * With `{ follow: true }` it reads the command's durable output live: the
   * retained chunks after the cursor `after` (default 0, the oldest), then new
   * ones as they are produced, each with its `cursor`. It ends when the
   * command has exited and all output was delivered. If the connection drops
   * it throws a `TransportError`; call again with `after` set to the last
   * `cursor` seen to continue without gaps or repeats. Throws `unsupported`
   * when the broker keeps no durable command logs.
   */
  logs(opts: { follow: true; after?: number }): AsyncGenerator<FollowedChunk, void, void>;
  logs(opts?: { follow?: false }): AsyncGenerator<{ stream: 'stdout' | 'stderr'; data: string }, void, void>;
  logs(
    opts: { follow?: boolean; after?: number } = {},
  ): AsyncGenerator<FollowedChunk | { stream: 'stdout' | 'stderr'; data: string }, void, void> {
    return opts.follow === true ? this.followedLogs(opts.after) : this.bufferedLogs();
  }

  private async *bufferedLogs(): AsyncGenerator<{ stream: 'stdout' | 'stderr'; data: string }, void, void> {
    const finished = await this.wait();
    yield* finished.logs();
  }

  private async *followedLogs(after: number | undefined): AsyncGenerator<FollowedChunk, void, void> {
    const decoders = { stdout: new TextDecoder(), stderr: new TextDecoder() };
    for await (const event of this.client.followLogs(this.sandboxId, this.cmdId, { afterSeq: after })) {
      switch (event.type) {
        case 'chunk': {
          const bytes = Uint8Array.from(event.data);
          yield {
            stream: event.stream,
            data: decoders[event.stream].decode(bytes, { stream: true }),
            bytes,
            cursor: event.log_seq,
          };
          break;
        }
        case 'reconnecting':
          // The gateway retries from the same cursor; nothing to surface.
          break;
        case 'resync_required':
          // Output was dropped by retention. Failing is the only way to keep a
          // consumer of plain chunks from reading a stream with a silent hole.
          throw new SandboxWsError(
            'resync_required',
            `command output was dropped after cursor ${event.from_cursor}; read it again from the start or accept the gap by following from the last cursor`,
            { leaseId: String(this.sandboxId) },
          );
        case 'unsupported':
          throw new SandboxWsError('unsupported', 'this broker does not keep durable command logs');
        case 'error':
          throw new SandboxWsError(event.code, event.message, { leaseId: String(this.sandboxId) });
        case 'end':
          return;
      }
    }
  }

  /**
   * Live output feed, only meaningful when this command was started with
   * `streamOutput: true`. Filters the lease's evidence stream down to this
   * execution's `command.<execution_id>` chunks and decodes each one; ends
   * when the underlying evidence stream ends (e.g. the client disconnects).
   * Throws if the command was not started with streaming enabled.
   */
  async *stream(): AsyncGenerator<{ stream: 'stdout' | 'stderr'; data: Uint8Array }, void, void> {
    if (!this.streaming) {
      throw new Error(`command ${this.cmdId} was not started with streamOutput: true`);
    }
    const suffix = `command.${this.cmdId}`;
    for await (const event of this.client.evidenceStream(this.sandboxId)) {
      if (event.subjectSuffix !== suffix) {
        continue;
      }
      const text = typeof event.payload === 'string' ? event.payload : new TextDecoder().decode(event.payload);
      const chunk = JSON.parse(text) as CommandOutputChunk;
      yield { stream: chunk.stream, data: Uint8Array.from(chunk.data) };
    }
  }
}

export class SandboxProcess {
  constructor(
    readonly name: string,
    private current: Command,
  ) {}

  get command(): Command {
    return this.current;
  }

  wait(opts?: Parameters<Command['wait']>[0]): Promise<CommandFinished> {
    return this.current.wait(opts);
  }

  stop(opts: { graceMs?: number } = {}): Promise<StopCommandResponse> {
    return this.current.stop(parseProcessInput(processStopOptionsSchema, opts));
  }

  async restart(opts: { graceMs?: number } = {}): Promise<void> {
    this.current = await this.current.restart(parseProcessInput(processStopOptionsSchema, opts));
  }

  logPage(opts?: Parameters<Command['logPage']>[0]) {
    return this.current.logPage(opts);
  }

  logs(opts: { follow: true; after?: number }): AsyncGenerator<FollowedChunk, void, void>;
  logs(opts?: { follow?: false }): AsyncGenerator<{ stream: 'stdout' | 'stderr'; data: string }, void, void>;
  logs(opts: { follow?: boolean; after?: number } = {}) {
    return opts.follow === true ? this.current.logs({ follow: true, after: opts.after }) : this.current.logs();
  }
}

function rejectOnAbort<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const onAbort = () => {
      signal.removeEventListener('abort', onAbort);
      reject(signal.reason);
    };
    signal.addEventListener('abort', onAbort, { once: true });
    promise.then(
      (value) => {
        signal.removeEventListener('abort', onAbort);
        resolve(value);
      },
      (err: unknown) => {
        signal.removeEventListener('abort', onAbort);
        reject(err);
      },
    );
    if (signal.aborted) onAbort();
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function attachExecutionIdentity(error: unknown, id: string): void {
  try {
    if (!(error instanceof Error) || !Object.isExtensible(error)) return;
    const descriptor = Object.getOwnPropertyDescriptor(error, 'executionId');
    if (
      descriptor === undefined ||
      ('value' in descriptor && descriptor.value === undefined && descriptor.writable === true)
    ) {
      Object.defineProperty(error, 'executionId', { value: id, enumerable: true });
    }
  } catch {}
}

function attachRestoreCleanup(error: unknown, receipt: RestoreCleanupReceipt): void {
  try {
    if (!(error instanceof Error) || !Object.isExtensible(error)) return;
    const descriptor = Object.getOwnPropertyDescriptor(error, 'restoreCleanup');
    if (
      descriptor === undefined ||
      ('value' in descriptor && descriptor.value === undefined && descriptor.writable === true)
    ) {
      Object.defineProperty(error, 'restoreCleanup', { value: receipt, enumerable: true });
    }
  } catch {}
}

function attachCommandCleanup(error: unknown, receipt: CommandCleanupReceipt): CommandCleanupReceipt {
  try {
    if (error instanceof Error && Object.isExtensible(error)) {
      const descriptor = Object.getOwnPropertyDescriptor(error, 'commandCleanup');
      if (
        descriptor === undefined ||
        ('value' in descriptor && descriptor.value === undefined && descriptor.writable === true)
      ) {
        Object.defineProperty(error, 'commandCleanup', { value: receipt, enumerable: true });
      }
    }
  } catch {}
  return receipt;
}

export interface WriteFileInput {
  path: string;
  content: string | Uint8Array;
  mode?: number;
}

/**
 * Machine size sugar. Each preset maps to a vCPU share and a memory size.
 * Presets never set a disk quota, because most providers cannot enforce one;
 * pass `diskMib` explicitly when you need it.
 *
 * | preset | vCPUs | `vcpu_millis` | `memory_mib` |
 * | ------ | ----- | ------------- | ------------ |
 * | small  | 1     | 1000          | 512          |
 * | medium | 2     | 2000          | 2048         |
 * | large  | 4     | 4000          | 8192         |
 */
export const MACHINE_SIZE_PRESETS = {
  small: { vcpu_millis: 1000, memory_mib: 512 },
  medium: { vcpu_millis: 2000, memory_mib: 2048 },
  large: { vcpu_millis: 4000, memory_mib: 8192 },
} as const;

export type MachineSizePreset = keyof typeof MACHINE_SIZE_PRESETS;

export interface CreateSandboxOptions {
  /** Machine size preset. Explicit `vcpus`, `memoryMib` and `diskMib` override it. */
  size?: MachineSizePreset;
  /** vCPU count; fractional values are allowed (0.5 is half a vCPU). Sent as `vcpu_millis`. */
  vcpus?: number;
  memoryMib?: number;
  /** Writable disk quota in MiB. */
  diskMib?: number;
  tenant: TenantId;
  /** Escape hatch: pass a full wire SandboxSpec directly instead of `image`. */
  spec?: SandboxSpec;
  image?: SandboxSpec['image'];
  name?: string;
  tags?: SandboxTags;
  /**
   * Adds the reserved ownership tag (`client=sandbox-ws-client`) so `list` and
   * `cleanup` can tell this SDK's leases from other tools' in a shared tenant.
   * Defaults to true. It uses one of the five tag slots and is skipped when
   * `tags` already has five or sets `client` itself. Pass `false` to opt out.
   */
  ownershipTag?: boolean;
  env?: Record<string, string>;
  /** Milliseconds; converted to whole seconds for the wire's `ttl_seconds`. */
  timeoutMs?: number;
  /** Releases the sandbox after this many milliseconds without activity (whole seconds, rounded up). */
  idleTimeoutMs?: number;
  /**
   * Staged idle handling measured from the last activity, for example pause
   * after 5 minutes and release after an hour. Each `afterMs` is rounded up to
   * whole seconds; stages must grow in time and in severity. Set this or
   * `idleTimeoutMs`, not both.
   */
  idlePolicy?: ReadonlyArray<{ afterMs: number; action: IdleAction }>;
  /**
   * Runs the sandbox with no wall-clock deadline; only cancel or the idle limit
   * ends it, so `idleTimeoutMs` or an `idlePolicy` release stage is required. The broker refuses it unless its
   * operator allowed no-TTL leases, and routes it only to providers with no
   * lifetime cap. `timeoutMs` still sizes the wall-time quota reservation.
   */
  noTtl?: boolean;
  /** Set false to purge the sandbox's command history when it ends. Defaults to true. */
  monitoring?: boolean;
  /** Set false to refuse every snapshot, fork or persistence capture of the sandbox's disk. Defaults to true. */
  snapshots?: boolean;
  networkPolicy?: NetworkPolicyInput;
  /**
   * Shorthand for an allow-list `networkPolicy`: `network: { allow: [...] }`.
   * Setting both `network` and `networkPolicy` is an error.
   */
  network?: NetworkAllowList;
  persistent?: boolean;
  /**
   * Vendor passthrough keyed by provider kind, for example
   * `{ daytona: { auto_stop_interval: 30 } }`. Denied unless the tenant is
   * allowed it, and only the named provider's worker sees its entry.
   */
  native?: Record<string, Record<string, unknown>>;
  /** Region to place the sandbox in. Fails with `no_worker_in_region` when no worker serves it. */
  region?: string;
  /** Regions to try, in order, when no worker serves `region`. Needs `region`. */
  fallbackRegions?: string[];
  /**
   * Ports to expose, resolvable with `sandbox.domain(port)` and
   * `sandbox.preview(port)`. Sets the spec's `exposed_ports`, replacing any
   * value already on a `spec` you pass. Up to 15 distinct ports, 1 to 65535.
   */
  ports?: number[];
  /**
   * Credentials to mask or withhold, sent as the spec's `secret_bindings`
   * (replacing any already on a `spec` you pass). Up to 64, with unique names
   * and targets, and no env target that `env` already sets.
   */
  secrets?: SecretBinding[];
  providerPref?: ProviderKind[];
  requiredCapabilities?: CapabilityRequirementsInput;
  /** Requires a browser-capable worker and an explicit operator-supported browser image. */
  browser?: boolean;
  idempotencyKey?: string;
  /**
   * Starts the sandbox from a snapshot taken with `sandbox.snapshot()`. Sets
   * the spec's image to that snapshot, so it cannot be combined with `image`.
   * Providers that cannot restore the snapshot fail the lease with a typed
   * denial (`no_worker_can_restore_snapshot`).
   */
  fromSnapshot?: RestorableSnapshot;
  /**
   * Records a change-tracking baseline right after the sandbox starts, so a later
   * `fs.changes()` with no `since` reports everything changed since creation. `true`
   * tracks `/`; `{ path }` tracks one subtree. Needs a provider with change tracking;
   * when the baseline cannot be recorded the new sandbox is stopped and the error thrown.
   */
  trackChanges?: boolean | { path: string };
  /** Requires broker operation cancellation. Confirmed cancellation never rolls back an already-completed create. */
  signal?: AbortSignal;
  /** 1 to 60000 ms for cancellation receipts, defaults to 10000. Pending intent remains observable by operation id. */
  cancellationTimeoutMs?: number;
}

const MAX_EXPOSED_PORTS = 15;
const MAX_SECRET_BINDINGS = 64;

const nonEmptyString = z.string().min(1);

const secretBindingSchema = z.object({
  name: z.string().regex(/^[A-Za-z0-9._-]+$/, 'must be non-empty and use only [A-Za-z0-9._-]'),
  target: z.union([
    z.strictObject({
      env: z
        .string()
        .regex(
          /^[A-Za-z_][A-Za-z0-9_]*$/,
          'must start with a letter or underscore and use only letters, digits and underscores',
        ),
    }),
    z.strictObject({ file: z.string().startsWith('/', 'must be an absolute path') }),
  ]),
  mode: z.enum(['mask', 'deny']),
  extract: z
    .union([
      z.strictObject({ regex: nonEmptyString }),
      z.strictObject({ json_key: z.array(nonEmptyString).min(1) }),
      z.strictObject({ env_line: nonEmptyString }),
      z.strictObject({
        jwt: z.strictObject({ mask_claims: z.array(nonEmptyString).optional(), pattern: nonEmptyString.optional() }),
      }),
    ])
    .optional(),
  mask_duplicates: z.boolean().optional(),
  on_extract_no_match: z.enum(['error', 'warn', 'deny']).optional(),
  strip_response_headers: z.array(z.string().regex(/^[A-Za-z0-9._-]+$/)).optional(),
  inject_hosts: z.array(nonEmptyString).min(1, 'needs at least one host'),
  source: z.looseObject({
    provider: nonEmptyString,
    pull_secret_id: nonEmptyString,
    scope: z.looseObject({ kind: nonEmptyString }),
    ttl_seconds: z.number().int().positive(),
  }),
});

const createOptionsSchema = z.object({
  ports: z
    .array(z.number().int().min(1).max(65535))
    .max(MAX_EXPOSED_PORTS)
    .refine((ports) => new Set(ports).size === ports.length, 'ports must be distinct')
    .optional(),
  secrets: z.array(secretBindingSchema).max(MAX_SECRET_BINDINGS).optional(),
  requiredCapabilities: capabilityRequirementsSchema.optional(),
  browser: z.boolean().optional(),
});

const attachmentMetadataSchema = z.object({
  name: z.string().min(1).optional(),
  tags: z.record(z.string(), z.string()).optional(),
  state: z.enum([
    'pending',
    'running',
    'paused',
    'suspending',
    'suspended',
    'waking',
    'cancelling',
    'released',
    'failed',
  ]),
  createdAt: z.date(),
});

const snapshotOptionsSchema = z.strictObject({
  acceptNative: z.boolean().optional(),
  expiresInMs: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER).optional(),
});

export type SnapshotOptions = z.input<typeof snapshotOptionsSchema>;

function attachmentMetadata(value: unknown) {
  const parsed = attachmentMetadataSchema.safeParse(value);
  if (!parsed.success) throw new SandboxWsError('invalid_response', 'invalid sandbox attachment metadata');
  return parsed.data;
}

function parseCreateOptions(
  opts: Pick<CreateSandboxOptions, 'ports' | 'secrets' | 'requiredCapabilities' | 'browser'>,
  env: Record<string, string>,
): { ports?: number[]; secrets?: SecretBinding[]; requiredCapabilities?: CapabilityRequirements; browser?: boolean } {
  const parsed = createOptionsSchema.safeParse({
    ports: opts.ports,
    secrets: opts.secrets,
    requiredCapabilities: opts.requiredCapabilities,
    browser: opts.browser,
  });
  if (!parsed.success) {
    const detail = parsed.error.issues.map((issue) => `${issue.path.join('.')}: ${issue.message}`).join('; ');
    throw new SandboxWsError('invalid_request', `invalid sandbox options: ${detail}`);
  }
  const names = new Set<string>();
  const targets = new Set<string>();
  for (const binding of opts.secrets ?? []) {
    const target = JSON.stringify(binding.target);
    if (names.has(binding.name)) {
      throw new SandboxWsError('invalid_request', `invalid sandbox options: duplicate secret name "${binding.name}"`);
    }
    if (targets.has(target)) {
      throw new SandboxWsError('invalid_request', `invalid sandbox options: duplicate secret target ${target}`);
    }
    names.add(binding.name);
    targets.add(target);
    if ('env' in binding.target && binding.target.env in env) {
      throw new SandboxWsError(
        'invalid_request',
        `invalid sandbox options: secret "${binding.name}" targets env var ${binding.target.env}, which env already sets`,
      );
    }
  }
  return { ...parsed.data, secrets: opts.secrets };
}

export interface ForkSandboxOptions {
  sourceSandbox: LeaseId | Sandbox;
  timeoutMs?: number;
  idempotencyKey?: string;
}

function isNativeSnapshot(
  source: LeaseSnapshotReceipt | OciArtifactRef | NativeSnapshotReceipt | NativeSnapshotRef,
): source is NativeSnapshotReceipt | NativeSnapshotRef {
  const ref = 'reference' in source ? source.reference : source;
  return 'provider' in ref;
}

function snapshotImage(
  source: LeaseSnapshotReceipt | OciArtifactRef | NativeSnapshotReceipt | NativeSnapshotRef,
): SandboxSpec['image'] {
  if (isNativeSnapshot(source)) {
    return { kind: 'native_snapshot', reference: 'reference' in source ? source.reference : source };
  }
  return { kind: 'snapshot', reference: 'reference' in source ? source.reference : source };
}

function defaultSpec(image: SandboxSpec['image'] | undefined, env: Record<string, string> | undefined): SandboxSpec {
  return {
    image: image ?? {
      kind: 'registry',
      reference: { registry: 'docker.io', repository: 'library/alpine', tag: 'latest' },
    },
    workspace: { empty: {} },
    env: env ?? {},
    cpu_seconds: 3600,
    memory_mib: 512,
    boot_mode: 'bare',
  };
}

/** A `Sandbox` whose provider is known to support `F`; what `sandbox.supports(F)` narrows to. */
export type SandboxWith<F extends SandboxFeature> = Sandbox & { readonly capabilities: SandboxCapabilities<F> };

/**
 * The ergonomic wrapper around a sandbox lease. Wraps a `SandboxClientApi`
 * (`SandboxWsClient`, or `MemorySandboxClient` in tests).
 */
export class Sandbox {
  readonly sandboxId: LeaseId;
  readonly provider: ProviderKind;
  readonly name?: string;
  readonly tags?: SandboxTags;
  /** State reported at attachment time, absent when the grant did not report it. */
  readonly state?: LeaseStatus;
  readonly createdAt?: Date;
  /**
   * True when this call created the sandbox, false when a named lease was
   * reconnected or the request was an idempotent replay. Gate one-time setup
   * (scaffolding, installs) on it. False for sandboxes from `get` and `list`.
   */
  readonly created: boolean;
  private _deadline: Date;
  private disposal: Promise<void> | undefined;
  private _capabilities: SandboxCapabilities | undefined;
  private readonly tracking: ChangeMarks = { marks: new Map() };
  private readonly servers = new Set<StartedServer>();
  private readonly serverRegistrations = new Map<
    StartedServer,
    { id: string; spec: ServerSpec; launched: LaunchedServer }
  >();
  private readonly environmentValues: Record<string, string> = Object.create(null);
  private readonly environmentNames = new Set<string>();

  constructor(
    private readonly client: SandboxClientApi,
    fields: {
      sandboxId: LeaseId;
      provider: ProviderKind;
      deadline: Date;
      created?: boolean;
      capabilities?: SandboxCapabilities;
      state?: LeaseStatus;
      createdAt?: Date;
    },
    opts?: { name?: string; tags?: SandboxTags },
  ) {
    this._capabilities = fields.capabilities;
    this.sandboxId = fields.sandboxId;
    this.provider = fields.provider;
    this._deadline = fields.deadline;
    this.created = fields.created ?? false;
    this.name = opts?.name;
    this.tags = opts?.tags;
    this.state = fields.state;
    this.createdAt = fields.createdAt;
  }

  get deadline(): Date {
    return this._deadline;
  }

  /**
   * What the sandbox's provider supports, when the broker reported it (the
   * `capabilities` field of the lease grant) or the caller supplied it with
   * `withCapabilities`. Undefined when unknown, which is what current brokers
   * produce for real providers.
   */
  get capabilities(): SandboxCapabilities | undefined {
    return this._capabilities;
  }

  /** Records the provider's capabilities on this handle and returns it. */
  withCapabilities(capabilities: SandboxCapabilities): this {
    this._capabilities = capabilities;
    return this;
  }

  /**
   * True only when the capabilities are known and include `feature`; false
   * for a provider that lacks it and for unknown capabilities, so check
   * `capabilities !== undefined` to tell the two apart. Narrows the handle's
   * `capabilities` type so a later `require` or branch carries the feature.
   */
  supports<F extends SandboxFeature>(feature: F): this is SandboxWith<F> {
    return this._capabilities?.has(feature) ?? false;
  }

  /**
   * Opens an interactive PTY terminal in this sandbox. Reads iterate raw
   * output; `write`, `resize` and `close` drive it. Throws when the provider
   * has no PTY support.
   */
  async terminal(opts: TerminalOptions = {}): Promise<Terminal> {
    const opened = await this.client.openTerminal({
      lease_id: this.sandboxId,
      cols: opts.cols ?? 80,
      rows: opts.rows ?? 24,
      ...(opts.cmd !== undefined ? { cmd: opts.cmd } : {}),
      ...(opts.env !== undefined ? { env: opts.env } : {}),
      ...(opts.cwd !== undefined ? { cwd: opts.cwd } : {}),
    });
    return new Terminal(this.client, opened.terminalId, opened.events);
  }

  get fs(): FileSystem {
    return new FileSystem(this.client, this.sandboxId, this.tracking);
  }

  /** git helpers with one-shot credential handling. */
  get git(): SandboxGit {
    return new SandboxGit(this);
  }

  static async create(client: SandboxClientApi, opts: CreateSandboxOptions): Promise<Sandbox> {
    opts.signal?.throwIfAborted();
    if (opts.signal === undefined) return Sandbox.createRequest(client, opts);
    const signal = opts.signal;
    const timeoutMs = cancellationTimeout(opts.cancellationTimeoutMs);
    if (typeof client.operationCapabilities !== 'function') {
      throw new SandboxWsError('unsupported', 'the broker does not expose operation cancellation');
    }
    const capability = parseOperationResponse(
      operationCapabilitiesSchema,
      await rejectOnAbort(client.operationCapabilities(), signal),
    );
    signal.throwIfAborted();
    if (!capability.supports_create_cancellation) {
      throw new SandboxWsError('unsupported', 'the broker does not support create cancellation');
    }
    const operationId = crypto.randomUUID();
    const creating = Sandbox.createRequest(client, opts, operationId);
    let onAbort!: () => void;
    const aborted = new Promise<never>((_resolve, reject) => {
      onAbort = () => {
        signal.removeEventListener('abort', onAbort);
        void cancelCreateOperation(client, operationId, opts.tenant, signal.reason, timeoutMs).catch(reject);
      };
      signal.addEventListener('abort', onAbort, { once: true });
      if (signal.aborted) onAbort();
    });
    void aborted.catch(() => {});
    const result = creating
      .then((sandbox) => (signal.aborted ? aborted : sandbox))
      .catch((error: unknown) => {
        if (signal.aborted) return aborted;
        throw error;
      });
    try {
      return await Promise.race([result, aborted]);
    } finally {
      signal.removeEventListener('abort', onAbort);
    }
  }

  private static async createRequest(
    client: SandboxClientApi,
    opts: CreateSandboxOptions,
    operationId?: string,
  ): Promise<Sandbox> {
    opts.signal?.throwIfAborted();
    const spec: SandboxSpec = { ...(opts.spec ?? defaultSpec(opts.image, opts.env)) };
    if (opts.fromSnapshot !== undefined) {
      if (opts.image !== undefined) {
        throw new SandboxWsError(
          'invalid_request',
          'invalid sandbox options: fromSnapshot cannot be combined with image',
        );
      }
      spec.image = snapshotImage(opts.fromSnapshot);
    }
    const clientState = snapshotClientState(opts.fromSnapshot);
    const typed = parseCreateOptions(opts, spec.env);
    if (typed.browser === true && opts.image === undefined && opts.spec?.image === undefined) {
      throw new SandboxWsError('invalid_request', 'browser requires an explicit operator-supported browser image');
    }
    const requiredCapabilities =
      typed.browser === true
        ? capabilityRequirementsSchema.parse({ ...typed.requiredCapabilities, needs_browser: true })
        : typed.requiredCapabilities;
    if (typed.ports !== undefined) {
      spec.exposed_ports = [...typed.ports];
    }
    if (typed.secrets !== undefined) {
      spec.secret_bindings = typed.secrets.map((binding) => ({ ...binding }));
    }
    const environmentNames = new Set([
      ...Object.keys(spec.env),
      ...(spec.secret_bindings ?? []).flatMap(({ target }) => ('env' in target ? [target.env] : [])),
    ]);
    requireServerEnvironment(clientState, spec.env, environmentNames);
    if (opts.network !== undefined && opts.networkPolicy !== undefined) {
      throw new SandboxWsError(
        'invalid_request',
        'invalid sandbox options: set either network or networkPolicy, not both',
      );
    }
    const networkPolicy = opts.networkPolicy ?? opts.network;
    if (networkPolicy !== undefined) {
      spec.network_policy = toWireNetworkPolicy(networkPolicy);
    }
    if (opts.size !== undefined) {
      const preset = MACHINE_SIZE_PRESETS[opts.size];
      spec.vcpu_millis = preset.vcpu_millis;
      spec.memory_mib = preset.memory_mib;
    }
    if (opts.vcpus !== undefined) {
      if (!Number.isFinite(opts.vcpus) || opts.vcpus <= 0) {
        throw new RangeError('vcpus must be a positive number');
      }
      spec.vcpu_millis = Math.max(1, Math.round(opts.vcpus * 1000));
    }
    if (opts.memoryMib !== undefined) {
      spec.memory_mib = opts.memoryMib;
    }
    if (opts.diskMib !== undefined) {
      spec.disk_mib = opts.diskMib;
    }
    if (opts.idleTimeoutMs !== undefined) {
      spec.idle_timeout_seconds = Math.ceil(opts.idleTimeoutMs / 1000);
    }
    if (opts.idlePolicy !== undefined) {
      if (opts.idleTimeoutMs !== undefined) {
        throw new RangeError('set idlePolicy or idleTimeoutMs, not both');
      }
      spec.idle_policy = {
        stages: opts.idlePolicy.map(({ afterMs, action }) => ({ after_seconds: Math.ceil(afterMs / 1000), action })),
      };
    }
    if (opts.noTtl === true) {
      const idleRelease = opts.idlePolicy?.some(({ action }) => action === 'release') ?? false;
      if (opts.idleTimeoutMs === undefined && !idleRelease) {
        throw new RangeError('noTtl needs idleTimeoutMs or an idlePolicy release stage');
      }
      spec.no_ttl = true;
    }
    if (opts.monitoring === false || opts.snapshots === false) {
      spec.retention = { monitoring: opts.monitoring ?? true, snapshots: opts.snapshots ?? true };
    }
    const tags = withOwnershipTag(opts.tags, opts.ownershipTag ?? true);
    const request: LeaseRequest = {
      ...(operationId === undefined ? {} : { operation_id: operationId }),
      tenant: opts.tenant,
      provider_pref: opts.providerPref ?? [],
      spec,
      ttl_seconds: opts.timeoutMs !== undefined ? Math.ceil(opts.timeoutMs / 1000) : 300,
      idempotency_key: opts.idempotencyKey ?? crypto.randomUUID(),
      host_imports: [],
      prewarm: false,
      tags,
      name: opts.name,
      persistent: opts.persistent,
      ...(requiredCapabilities === undefined ? {} : { required_capabilities: requiredCapabilities }),
      ...(opts.native === undefined ? {} : { native: opts.native }),
      ...(opts.region === undefined ? {} : { region: opts.region }),
      ...(opts.fallbackRegions === undefined ? {} : { fallback_regions: opts.fallbackRegions }),
    };
    const leasing = client.lease(request);
    let granted: LeaseGranted;
    try {
      granted =
        opts.signal === undefined || operationId !== undefined
          ? await leasing
          : await rejectOnAbort(leasing, opts.signal);
    } catch (error) {
      if (opts.signal?.aborted && operationId === undefined) {
        void leasing
          .then((late) => (late.created === true ? client.deleteLease(late.lease_id) : undefined))
          .catch(() => {});
      }
      throw error;
    }
    const sandbox = Sandbox.fromLeaseGranted(client, granted, { name: opts.name, tags });
    sandbox.rememberEnvironment(spec.env, environmentNames);
    const provisioning = opts.signal === undefined ? undefined : new AbortController();
    const abortProvisioning = () => provisioning?.abort(opts.signal?.reason);
    opts.signal?.addEventListener('abort', abortProvisioning, { once: true });
    if (opts.signal?.aborted) abortProvisioning();
    const initializing = (async () => {
      opts.signal?.throwIfAborted();
      if (opts.trackChanges !== undefined && opts.trackChanges !== false) {
        const path = opts.trackChanges === true ? '/' : opts.trackChanges.path;
        const { checkpointId } = await client.checkpointFiles(sandbox.sandboxId, path);
        opts.signal?.throwIfAborted();
        sandbox.tracking.baseline = checkpointId;
      }
      if (clientState !== undefined) {
        const continuity = granted.created === true ? snapshotExecutionContinuity(opts.fromSnapshot!) : 'unknown';
        await sandbox.restoreServers(clientState, continuity, provisioning?.signal);
      }
      opts.signal?.throwIfAborted();
      return sandbox;
    })();
    try {
      const initialized =
        opts.signal === undefined || operationId !== undefined
          ? await initializing
          : await rejectOnAbort(initializing, opts.signal);
      opts.signal?.throwIfAborted();
      return initialized;
    } catch (error) {
      if (granted.created === true && !(operationId !== undefined && opts.signal?.aborted))
        await client.deleteLease(granted.lease_id).catch(() => {});
      throw error;
    } finally {
      opts.signal?.removeEventListener('abort', abortProvisioning);
    }
  }

  /**
   * Starts a sandbox from a snapshot receipt. Same as `Sandbox.create` with
   * `fromSnapshot`.
   */
  static async fromSnapshot(
    client: SandboxClientApi,
    receipt: RestorableSnapshot,
    opts: Omit<CreateSandboxOptions, 'fromSnapshot' | 'image'>,
  ): Promise<Sandbox> {
    return Sandbox.create(client, { ...opts, fromSnapshot: receipt });
  }

  /** Deletes a snapshot by digest. Resolves false when it did not exist (a repeat is not an error). */
  static async deleteSnapshot(client: SandboxClientApi, digest: Digest): Promise<boolean> {
    const response = await client.snapshotDelete(digest);
    if (response.outcome === 'unsupported') {
      throw new SandboxWsError('unsupported', 'the broker cannot delete snapshots');
    }
    return response.outcome === 'deleted';
  }

  /** Retires an owned native reference. Resolves false after confirmed prior deletion. */
  static async deleteNativeSnapshot(client: SandboxClientApi, reference: NativeSnapshotRef): Promise<boolean> {
    const response = await client.nativeSnapshotDelete(reference);
    if (response.outcome === 'unsupported') {
      throw new SandboxWsError('unsupported', 'the provider cannot delete native snapshots');
    }
    return response.outcome === 'deleted';
  }

  static async fork(client: SandboxClientApi, opts: ForkSandboxOptions): Promise<Sandbox> {
    const sourceLeaseId = opts.sourceSandbox instanceof Sandbox ? opts.sourceSandbox.sandboxId : opts.sourceSandbox;
    const ttlSeconds = opts.timeoutMs !== undefined ? Math.ceil(opts.timeoutMs / 1000) : 300;
    const granted = await client.fork(sourceLeaseId, ttlSeconds, opts.idempotencyKey ?? crypto.randomUUID());
    return Sandbox.fromLeaseGranted(client, granted);
  }

  private static fromLeaseGranted(
    client: SandboxClientApi,
    granted: LeaseGranted,
    opts?: { name?: string; tags?: SandboxTags },
  ): Sandbox {
    return new Sandbox(
      client,
      {
        sandboxId: granted.lease_id,
        provider: granted.provider,
        deadline: wireSystemTimeToDate(granted.deadline),
        created: granted.created ?? false,
        capabilities: granted.capabilities === undefined ? undefined : SandboxCapabilities.from(granted.capabilities),
      },
      opts,
    );
  }

  /** Looks up a running lease by its tenant-scoped `name` (see `LeaseRequest.name`). Throws if not found. */
  static async get(client: SandboxClientApi, opts: { name: string } & ServerConnectionOptions): Promise<Sandbox> {
    const state = opts.clientState === undefined ? undefined : parseServerClientState(opts.clientState);
    const response = await client.getLeaseByName(opts.name);
    if (response.outcome === 'not_found') {
      throw new Error(`sandbox with name "${opts.name}" not found`);
    }
    const sandbox = Sandbox.fromLeaseByNameRow(client, response.row);
    sandbox.attachServerState(state, opts.env);
    return sandbox;
  }

  /**
   * Permanently deletes the sandbox called `name`, even when it has no active
   * lease (a stopped named sandbox). Resolves `false` when nothing existed
   * under that name.
   */
  static async deleteByName(client: SandboxClientApi, name: string): Promise<boolean> {
    const response = await client.deleteNamedSandbox(name);
    return response.outcome === 'deleted';
  }

  /**
   * Attaches to an existing lease by id, for dashboards and support tools
   * that hold lease ids. It looks the id up among the session tenant's leases
   * (the same tenant-scoped verb as `list`, with no ownership filter), so a
   * lease of another tenant is indistinguishable from one that does not exist.
   * Throws `SandboxNotFoundError` (code `lease_not_found`) in both cases.
   */
  static async connect(
    client: SandboxClientApi,
    sandboxId: LeaseId | string,
    opts: ServerConnectionOptions = {},
  ): Promise<Sandbox> {
    const state = opts.clientState === undefined ? undefined : parseServerClientState(opts.clientState);
    const { leases } = await client.listLeases();
    const row = leases.find((candidate) => candidate.lease_id === sandboxId);
    if (row === undefined) {
      throw new SandboxNotFoundError(sandboxId);
    }
    const sandbox = Sandbox.fromLeaseListRow(client, row);
    sandbox.attachServerState(state, opts.env);
    return sandbox;
  }

  /**
   * Lists the session tenant's leases that carry the ownership tag, so a
   * tenant shared with other tools shows only what this SDK created. `tags`
   * narrows further. Pass `includeForeign: true` to list every lease of the
   * tenant, which is also the way to see leases created before the ownership
   * tag existed, with `ownershipTag: false`, or by `fork`.
   */
  static async list(
    client: SandboxClientApi,
    opts?: { tags?: SandboxTags; includeForeign?: boolean },
  ): Promise<Sandbox[]> {
    const tags = ownershipFilter(opts?.tags, opts?.includeForeign);
    const { leases } = await client.listLeases(tags === undefined ? undefined : { tags });
    return leases.map((row) => Sandbox.fromLeaseListRow(client, row));
  }

  /**
   * Releases leases matching every filter, or only reports them with
   * `dryRun`. Like `list` it is limited to leases with the ownership tag
   * unless `includeForeign` is set, so a bare call reaps only this SDK's
   * leases. With `includeForeign` and no other filter the broker rejects the
   * call, since reaping needs at least one filter. Safe to repeat.
   */
  static async cleanup(
    client: SandboxClientApi,
    opts: {
      tags?: SandboxTags;
      olderThanMs?: number;
      idleForMs?: number;
      dryRun?: boolean;
      includeForeign?: boolean;
    } = {},
  ): Promise<ReapLeasesResponse> {
    const tags = ownershipFilter(opts.tags, opts.includeForeign);
    return client.reapLeases({
      ...(tags === undefined || Object.keys(tags).length === 0 ? {} : { tags }),
      ...(opts.olderThanMs === undefined ? {} : { older_than_seconds: Math.ceil(opts.olderThanMs / 1000) }),
      ...(opts.idleForMs === undefined ? {} : { idle_for_seconds: Math.ceil(opts.idleForMs / 1000) }),
      ...(opts.dryRun === undefined ? {} : { dry_run: opts.dryRun }),
    });
  }

  private static fromLeaseByNameRow(client: SandboxClientApi, row: LeaseByNameRow): Sandbox {
    const metadata = attachmentMetadata({
      name: row.name,
      tags: row.tags,
      state: row.status,
      createdAt: wireSystemTimeToDate(row.created_at),
    });
    return new Sandbox(
      client,
      {
        sandboxId: row.lease_id as LeaseId,
        provider: row.provider,
        deadline: wireSystemTimeToDate(row.deadline),
        state: metadata.state,
        createdAt: metadata.createdAt,
      },
      { name: metadata.name, tags: metadata.tags },
    );
  }

  private static fromLeaseListRow(client: SandboxClientApi, row: LeaseListRow): Sandbox {
    const metadata = attachmentMetadata({
      name: row.name,
      tags: row.tags,
      state: row.state,
      createdAt: new Date(row.started_at * 1000),
    });
    return new Sandbox(
      client,
      {
        sandboxId: row.lease_id as LeaseId,
        provider: row.provider,
        deadline: new Date((row.started_at + row.ttl_seconds) * 1000),
        state: metadata.state,
        createdAt: metadata.createdAt,
      },
      { name: metadata.name, tags: metadata.tags },
    );
  }

  /**
   * The vendor's own id (and URL, when it has one) for this sandbox, so it
   * can be reached with the vendor's SDK. Needs native passthrough for the
   * tenant. Throws when the provider has no such handle.
   */
  async nativeHandle(): Promise<{ provider: ProviderKind; vendorId: string; url?: string; region?: string }> {
    const response = await this.client.getNativeHandle(this.sandboxId);
    if (response.outcome === 'available') {
      return {
        provider: response.provider,
        vendorId: response.vendor_id,
        ...(response.url === undefined ? {} : { url: response.url }),
        ...(response.region === undefined ? {} : { region: response.region }),
      };
    }
    if (response.outcome === 'unsupported') {
      throw new Error(`provider ${this.provider} has no native handle`);
    }
    throw new Error(`sandbox.nativeHandle() denied for sandbox ${this.sandboxId}`);
  }

  /**
   * Endpoint and credential for an SSH session straight to this sandbox,
   * minted by the provider (Daytona today). Needs native passthrough for the
   * tenant. Throws when the provider has no SSH endpoint.
   */
  async sshAccess(options: { ttlSeconds?: number } = {}): Promise<SshAccess> {
    const response = await this.client.getSshAccess(this.sandboxId, options.ttlSeconds);
    if (response.outcome === 'available') {
      const { outcome: _outcome, ...access } = response;
      return access;
    }
    if (response.outcome === 'unsupported') {
      throw new Error(`provider ${this.provider} has no ssh access`);
    }
    throw new Error(`sandbox.sshAccess() denied for sandbox ${this.sandboxId}`);
  }

  /**
   * Resolves a port from the sandbox's `exposed_ports` into a reachable
   * `host:port` endpoint.
   * Where the provider supports the `expose_after_create` feature, a port
   * not declared at create resolves too, e.g. a dev server started later.
   * Throws a descriptive error for `port_not_exposed`/`unsupported`/`denied`
   * rather than returning a union the caller has to narrow.
   */
  async domain(port: number): Promise<string> {
    const response = await this.client.getSandboxDomain(this.sandboxId, port);
    if (response.outcome === 'available') {
      return response.endpoint;
    }
    if (response.outcome === 'port_not_exposed') {
      throw new Error(`port ${port} is not exposed on sandbox ${this.sandboxId}`);
    }
    if (response.outcome === 'unsupported') {
      throw new Error(`provider ${this.provider} does not support sandbox.domain()`);
    }
    throw new Error(`sandbox.domain(${port}) denied for sandbox ${this.sandboxId}`);
  }

  /**
   * Resolves a port into an authenticated, expiring preview URL. The URL
   * carries a signed token bound to this tenant, lease and port, so unlike the
   * bare endpoint from {@link domain} it is safe to hand to a browser. Throws
   * when the gateway has preview access control disabled (it then returns only
   * the bare endpoint).
   *
   * The URL looks like `<gateway>/v1/preview/<lease>/<port>/<token>/` and
   * proxies every path below it to the port. It stops working at `expiresAt`;
   * call `preview` again for a fresh URL, which does not invalidate earlier
   * ones. Docker and similar providers need `ports` declared at create;
   * Daytona serves any port of a running sandbox.
   *
   * With `waitMs` the gateway holds the reply until the port answers HTTP and
   * throws {@link PreviewNotReadyError} (retryable) when it does not in time.
   */
  async preview(port: number, options: { waitMs?: number } = {}): Promise<{ url: string; expiresAt: Date }> {
    const waitMs = options.waitMs;
    if (waitMs !== undefined && (!Number.isFinite(waitMs) || waitMs < 0)) {
      throw new RangeError('waitMs must be a non-negative number of milliseconds');
    }
    const response = await this.client.getSandboxDomain(this.sandboxId, port, waitMs);
    if (response.outcome === 'not_ready') {
      throw new PreviewNotReadyError(port, response.waited_ms, { leaseId: this.sandboxId, provider: this.provider });
    }
    if (response.outcome === 'available') {
      if (response.url === undefined || response.expires_at === undefined) {
        throw new Error('preview access control is not enabled on this gateway');
      }
      return { url: response.url, expiresAt: new Date(response.expires_at * 1000) };
    }
    if (response.outcome === 'port_not_exposed') {
      throw new Error(`port ${port} is not exposed on sandbox ${this.sandboxId}`);
    }
    if (response.outcome === 'unsupported') {
      throw new Error(`provider ${this.provider} does not support sandbox.preview()`);
    }
    throw new Error(`sandbox.preview(${port}) denied for sandbox ${this.sandboxId}`);
  }

  runCommand(cmd: string, args?: string[]): Promise<CommandFinished>;
  runCommand(params: RunCommandParams & { detached: true }): Promise<Command>;
  runCommand(params: RunCommandParams & { detached?: false }): Promise<CommandFinished>;
  async runCommand(cmdOrParams: string | RunCommandParams, args?: string[]): Promise<Command | CommandFinished> {
    const params: RunCommandParams = typeof cmdOrParams === 'string' ? { cmd: cmdOrParams, args } : cmdOrParams;
    const signal = params.signal;
    signal?.throwIfAborted();
    const cancellationBudget = cancellationTimeout(params.cancellationTimeoutMs);
    const supportsCancellation = this.supports('command_cancellation');
    const supportsLaunchCancellation = supportsCancellation && this.supports('command_launch_cancellation');
    const named = params.process?.name !== undefined;
    if (signal !== undefined && (!supportsCancellation || (named && !supportsLaunchCancellation))) {
      throw new SandboxWsError(
        'unsupported',
        'signal cancellation requires verified cancellation support; named commands also require launch cancellation',
      );
    }
    const launchKey = signal !== undefined && supportsLaunchCancellation ? crypto.randomUUID() : undefined;
    const startedAtMs = Date.now();
    const started = this.client.runCommand(this.sandboxId, [params.cmd, ...(params.args ?? [])], {
      cwd: params.cwd,
      env: withNonInteractiveEnv(params.env, params),
      detached: signal === undefined ? params.detached : true,
      streamOutput: params.streamOutput,
      runAsUser: params.user,
      stdin: params.stdin,
      outputRetention: params.outputRetention,
      timeoutSeconds: params.timeoutSeconds,
      deadline: params.deadline,
      maxOutputBytes: params.maxOutputBytes,
      process: params.process,
      redactValues: params.redactValues,
      launchKey,
      requireLaunchCancellation: launchKey === undefined ? undefined : true,
    });
    if (signal === undefined) {
      return Command.fromRunCommandResponse(
        this.client,
        this.sandboxId,
        await started,
        params.streamOutput ?? false,
        Date.now() - startedAtMs,
        { supported: supportsCancellation, named },
      );
    }
    let abortedAt: number | undefined;
    let knownExecution: string | undefined;
    let canceledBeforeAcknowledgement = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const acknowledged = started.then((value) => {
      const response = cancellationRunResponse(value);
      if (response.outcome === 'started') knownExecution = response.execution_id;
      const remaining = abortedAt === undefined ? cancellationBudget : cancellationBudget - (Date.now() - abortedAt);
      return Command.fromRunCommandResponse(
        this.client,
        this.sandboxId,
        response,
        params.streamOutput ?? false,
        Date.now() - startedAtMs,
        {
          supported: true,
          signal: canceledBeforeAcknowledgement ? undefined : signal,
          timeoutMs: remaining > 0 ? remaining : cancellationBudget,
          launchKey,
          named,
        },
      );
    });
    let onAbort!: () => void;
    const aborted = new Promise<never>((_resolve, reject) => {
      const incomplete = () =>
        new CommandCancellationError(this.sandboxId, knownExecution, signal.reason, true, { launchKey });
      onAbort = () => {
        signal.removeEventListener('abort', onAbort);
        abortedAt = Date.now();
        if (launchKey !== undefined && knownExecution === undefined) {
          canceledBeforeAcknowledgement = true;
          void cancelPendingCommandLaunch(
            this.client,
            this.sandboxId,
            launchKey,
            signal.reason,
            cancellationBudget,
          ).catch(reject);
          return;
        }
        timer = setTimeout(() => reject(incomplete()), cancellationBudget);
        void acknowledged
          .then(
            async (command) => {
              if (command instanceof Command) await command.cancel();
              clearTimeout(timer);
              reject(new CommandCancellationError(this.sandboxId, knownExecution, signal.reason, false, { launchKey }));
            },
            () => {
              clearTimeout(timer);
              reject(incomplete());
            },
          )
          .catch((error: unknown) => {
            clearTimeout(timer);
            reject(error instanceof CommandCancellationError ? error : incomplete());
          });
      };
      signal.addEventListener('abort', onAbort, { once: true });
      if (signal.aborted) onAbort();
    });
    void aborted.catch(() => {});
    const result = acknowledged
      .then(async (command) => {
        if (signal.aborted) return aborted;
        return params.detached === true || command instanceof CommandFinished ? command : await command.wait();
      })
      .catch((error: unknown) => {
        if (signal.aborted) return aborted;
        throw launchKey === undefined ? error : retainCommandLaunchAuthority(error, this.sandboxId, launchKey);
      });
    try {
      return await Promise.race([result, aborted]);
    } finally {
      signal.removeEventListener('abort', onAbort);
      clearTimeout(timer);
    }
  }

  /**
   * Tagged template that runs a shell command with every interpolation quoted
   * as one word: `` await sandbox.$`grep -r ${pattern} ${dirs}` `` runs
   * `sh -c` and resolves to a `CommandFinished` (call `.throwIfFailed()` to
   * turn a non-zero exit into an error). Arrays expand to one word per
   * element; wrap trusted syntax such as a pipe in `raw()`. Use `shellWith`
   * for cwd, env or a timeout.
   */
  get $(): (strings: TemplateStringsArray, ...values: unknown[]) => Promise<CommandFinished> {
    return this.shellWith();
  }

  /** Like `$`, with `RunCommandParams` applied to the run (everything except `cmd`, `args` and `detached`). */
  shellWith(
    params: Omit<RunCommandParams, 'cmd' | 'args' | 'detached'> = {},
  ): (strings: TemplateStringsArray, ...values: unknown[]) => Promise<CommandFinished> {
    return async (strings, ...values) =>
      this.runCommand({ ...params, cmd: 'sh', args: ['-c', shellTemplate(strings, ...values)], detached: false });
  }

  createLspServer(spec: LspServerSpec | LspPresetName, projectDir: string, opts?: LspServerOptions): LspServer {
    const resolved = typeof spec === 'string' ? LSP_PRESETS[spec] : spec;
    return new LspServer(this.client, this, resolved, projectDir, opts);
  }

  async getCommand(cmdId: string): Promise<Command> {
    return new Command(
      this.client,
      this.sandboxId,
      cmdId,
      false,
      {},
      { supported: this.supports('command_cancellation') },
    );
  }

  // Sugar: there is no dedicated mkdir wire verb, so this shells out to `mkdir -p`.
  async mkDir(path: string, opts?: { user?: string }): Promise<void> {
    const result = await this.runCommand({ cmd: 'mkdir', args: ['-p', path], user: opts?.user });
    if (result.exitCode !== 0) {
      throw new Error(`mkDir failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
  }

  async readFile(opts: { path: string; user?: string }): Promise<string | null> {
    const bytes = await this.readFileToBuffer(opts);
    return bytes === null ? null : new TextDecoder().decode(bytes);
  }

  // A missing file is the typed `not_found` outcome. The message match below
  // only serves older gateways, which fail the request with the worker's own
  // text instead.
  async readFileToBuffer(opts: { path: string; user?: string }): Promise<Uint8Array | null> {
    try {
      const response = await this.client.readFile(this.sandboxId, opts.path, { asUser: opts.user });
      if (response.outcome === 'not_found') {
        return null;
      }
      return decodeBytes(response.data);
    } catch (err) {
      if (isFileNotFoundError(err)) {
        return null;
      }
      throw err;
    }
  }

  async writeFiles(files: WriteFileInput[], opts?: { user?: string }): Promise<void> {
    const encoder = new TextEncoder();
    const wireFiles = files.map((file) => ({
      path: file.path,
      content: Array.from(typeof file.content === 'string' ? encoder.encode(file.content) : file.content),
      mode: file.mode,
    }));
    await this.client.writeFiles(this.sandboxId, wireFiles, { asUser: opts?.user });
  }

  async stop(): Promise<void> {
    await this.client.stopLease(this.sandboxId);
  }

  async start(params: StartProcessParams): Promise<SandboxProcess> {
    const { name: suppliedName, readiness, readyTimeoutMs, ...run } = parseProcessInput(startProcessSchema, params);
    run.signal?.throwIfAborted();
    const name = suppliedName ?? (await defaultProcessName(run.cmd, run.args ?? []));
    const command = await this.runNamedCommand({
      ...run,
      process: { name, ...(readiness === undefined ? {} : { readiness }) },
    });
    try {
      run.signal?.throwIfAborted();
      if (!(command instanceof Command))
        throw new SandboxWsError('invalid_response', 'a named launch did not return a detached command');
      if (readiness !== undefined) {
        const waiting = this.client.waitReady(
          this.sandboxId,
          command.cmdId,
          Math.ceil((readyTimeoutMs ?? 30_000) / 1000),
        );
        const response = parseProcessReady(
          run.signal === undefined ? await waiting : await rejectOnAbort(waiting, run.signal),
        );
        if (response.outcome === 'failed')
          throw new ServerExitedError(response.exit_code, '', { leaseId: this.sandboxId });
        if (response.outcome === 'not_ready')
          throw new TimeoutError('named process did not become ready within the requested timeout', {
            leaseId: this.sandboxId,
          });
        if (response.outcome === 'not_found')
          throw new SandboxWsError('not_found', 'named process was not found during readiness', {
            leaseId: this.sandboxId,
          });
        if (response.outcome === 'unsupported' || response.outcome === 'no_probe') {
          throw new SandboxWsError('unsupported', 'the worker cannot report the requested process readiness', {
            leaseId: this.sandboxId,
          });
        }
      }
      run.signal?.throwIfAborted();
      return new SandboxProcess(name, command);
    } catch (error) {
      attachExecutionIdentity(error, command.cmdId);
      await this.cleanupFailedCommand(command, error, command.origin === 'created');
      throw error;
    }
  }

  async processes(): Promise<ProcessInfo[]> {
    const response = parseServerProcessInventory(await this.client.listProcesses(this.sandboxId));
    if (response.outcome === 'unsupported')
      throw new SandboxWsError('unsupported', 'the worker does not support process discovery');
    return response.processes;
  }

  async process(name: string): Promise<SandboxProcess | null> {
    const target = parseProcessInput(processNameSchema, name);
    const matches = (await this.processes()).filter((process) => process.name === target);
    const current =
      matches.find((process) => process.state === 'running') ?? (matches.length === 1 ? matches[0] : undefined);
    if (current === undefined && matches.length > 1) {
      throw new SandboxWsError(
        'invalid_response',
        'multiple exited executions share the name; select an execution from the inventory',
      );
    }
    return current === undefined
      ? null
      : new SandboxProcess(
          target,
          new Command(
            this.client,
            this.sandboxId,
            current.execution_id,
            false,
            {},
            { supported: this.supports('command_cancellation') },
          ),
        );
  }

  /**
   * Resolves once the sandbox is usable: it is running, which the broker
   * records only after the workspace was materialized, and every port in
   * `opts.ports` accepts a connection inside the sandbox. The broker does the
   * waiting, so there is no poll loop on this side beyond re-asking in slices.
   * Throws `TimeoutError` naming the ports that never answered when
   * `opts.timeoutMs` (default 30 s, at most 5 min) elapses, and `unsupported`
   * when ports were requested and the provider cannot exec or the image has neither bash nor nc. At most
   * {@link MAX_READY_PORTS} ports.
   */
  async waitUntilReady(opts: WaitUntilReadyOptions = {}): Promise<void> {
    const ports = opts.ports ?? [];
    if (ports.length > MAX_READY_PORTS) {
      throw new SandboxWsError('invalid_payload', `at most ${MAX_READY_PORTS} ports may be checked`, {
        leaseId: this.sandboxId,
      });
    }
    const timeoutMs = Math.min(opts.timeoutMs ?? DEFAULT_READY_TIMEOUT_MS, MAX_WAIT_SANDBOX_READY_SECONDS * 1000);
    const deadline = Date.now() + timeoutMs;
    let pending: number[] = [];
    for (;;) {
      const remainingMs = Math.max(deadline - Date.now(), 0);
      const sliceSeconds = Math.ceil(Math.min(remainingMs, READY_SLICE_MS) / 1000);
      const response = await this.client.waitSandboxReady(
        this.sandboxId,
        ports.length > 0 ? ports : undefined,
        sliceSeconds,
      );
      if (response.outcome === 'ready') {
        return;
      }
      if (response.outcome === 'unsupported') {
        throw new SandboxWsError('unsupported', `provider ${this.provider} cannot check ports in a sandbox`, {
          leaseId: this.sandboxId,
        });
      }
      pending = response.pending_ports;
      if (Date.now() >= deadline) {
        const what = pending.length > 0 ? `ports ${pending.join(', ')} to accept connections` : 'it to start running';
        throw new TimeoutError(`timed out waiting for sandbox ${this.sandboxId}: ${what}`, { leaseId: this.sandboxId });
      }
    }
  }

  /**
   * Starts `cmd` in the background, waits until `port` accepts connections
   * (and `path` answers, when given) and resolves its preview URL, so one call
   * takes a dev server from nothing to a link.
   *
   * The port check is the broker's (`waitUntilReady`), using bash or nc in
   * the image. It runs in one second slices so a process that dies is noticed
   * at once. The path check runs `curl` or `wget` inside the sandbox, because a
   * preview wait cannot follow a path. When the process exits first the call
   * throws {@link ServerExitedError}; when `timeoutMs` passes it throws
   * {@link TimeoutError}. Both carry the tail of the process output, and the
   * cleanup targets unnamed work and provider-confirmed new named work.
   * Reused or unknown named executions are preserved.
   * Extensible errors carry `executionId` for intentional cleanup.
   */
  async startServer(params: StartServerParams): Promise<StartedServer> {
    const { port, path, name, preview, timeoutMs, restartOnResume, ...run } = params;
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      throw new RangeError('port must be an integer from 1 to 65535');
    }
    if (path !== undefined && !/^\/\S*$/.test(path)) {
      throw new RangeError('path must start with "/" and contain no whitespace');
    }
    const budgetMs = timeoutMs ?? DEFAULT_START_SERVER_TIMEOUT_MS;
    if (!Number.isFinite(budgetMs) || budgetMs < 0) {
      throw new RangeError('timeoutMs must be a non-negative number of milliseconds');
    }
    const spec: ServerSpec = { run, port, path, name, preview, budgetMs };
    const launched = await this.launchServer(spec);
    return this.registerServer(spec, launched, restartOnResume !== false);
  }

  private registerServer(
    spec: ServerSpec,
    launched: LaunchedServer,
    restartOnResume = true,
    id: string = crypto.randomUUID(),
  ): StartedServer {
    const { name, port } = spec;
    const server: StartedServer = {
      get command() {
        return launched.command;
      },
      name,
      port,
      get url() {
        return launched.url;
      },
      get expiresAt() {
        return launched.expiresAt;
      },
      restart: async () => {
        await launched.command.kill('term').catch(() => {});
        const next = await this.launchServer(spec);
        launched.command = next.command;
        launched.url = next.url;
        launched.expiresAt = next.expiresAt;
        if (restartOnResume) {
          this.servers.add(server);
          this.serverRegistrations.set(server, { id, spec, launched });
        }
      },
      stop: async () => {
        this.servers.delete(server);
        this.serverRegistrations.delete(server);
        await launched.command.kill('term').catch(() => {});
      },
    };
    if (restartOnResume) {
      this.servers.add(server);
      this.serverRegistrations.set(server, { id, spec, launched });
    }
    return server;
  }

  private rememberEnvironment(values: Record<string, string>, names: Iterable<string> = Object.keys(values)): void {
    Object.assign(this.environmentValues, values);
    for (const name of names) this.environmentNames.add(name);
  }

  /** Caller-owned restart intent for reconnecting a handle. Environment values are never exported. */
  exportClientState(): ServerClientState {
    const servers: ServerRestartEntry[] = [];
    for (const server of this.servers) {
      const registration = this.serverRegistrations.get(server)!;
      const {
        env,
        redactValues = [],
        signal: _signal,
        cancellationTimeoutMs: _cancellationTimeoutMs,
        ...run
      } = registration.spec.run;
      const environment = { ...this.environmentValues, ...env };
      const environmentKeys = [...new Set([...this.environmentNames, ...Object.keys(env ?? {})])];
      const redactedEnvironmentKeys = environmentKeys.filter(
        (key) => environment[key] !== undefined && redactValues.includes(environment[key]!),
      );
      const { port, path, name, preview, budgetMs } = registration.spec;
      const params = { ...run, port, path, name, preview, timeoutMs: budgetMs };
      const persisted = [
        params.cmd,
        ...(params.args ?? []),
        params.cwd,
        params.user,
        params.path,
        params.name,
        registration.id,
        server.command.cmdId,
        this.sandboxId,
      ];
      if (
        redactValues.some(
          (value) =>
            persisted.some((text) => text?.includes(value)) ||
            !redactedEnvironmentKeys.some((key) => environment[key] === value),
        )
      ) {
        throw new SandboxWsError(
          'invalid_request',
          'server restart state cannot retain literal redaction values; provide environment replacements',
        );
      }
      servers.push({
        id: registration.id,
        executionId: server.command.cmdId,
        params,
        environmentKeys,
        overrideEnvironmentKeys: Object.keys(env ?? {}),
        redactedEnvironmentKeys,
      });
    }
    return parseServerClientState({ version: 1, sourceSandboxId: this.sandboxId, servers });
  }

  private restoredServerSpec(entry: ServerRestartEntry): ServerSpec {
    const { port, path, name, preview, timeoutMs, ...run } = entry.params;
    const env = Object.fromEntries(
      entry.overrideEnvironmentKeys
        .filter((key) => Object.hasOwn(this.environmentValues, key))
        .map((key) => [key, this.environmentValues[key]!]),
    );
    const redactValues = entry.redactedEnvironmentKeys.map((key) => this.environmentValues[key]!);
    return {
      run: {
        ...run,
        ...(Object.keys(env).length === 0 ? {} : { env }),
        ...(redactValues.length === 0 ? {} : { redactValues }),
      },
      port,
      path,
      name,
      preview,
      budgetMs: timeoutMs,
    };
  }

  private attachServerState(state: ServerClientState | undefined, env: Record<string, string> = {}): void {
    if (state === undefined) return;
    if (state.sourceSandboxId !== this.sandboxId) {
      throw new SandboxWsError('invalid_request', 'server restart state belongs to a different sandbox');
    }
    requireServerEnvironment(state, env, new Set(Object.keys(env)), state.sourceSandboxId === this.sandboxId);
    this.rememberEnvironment(env);
    for (const entry of state.servers) {
      for (const key of entry.environmentKeys) this.environmentNames.add(key);
      this.registerServer(
        this.restoredServerSpec(entry),
        {
          command: new Command(
            this.client,
            this.sandboxId,
            entry.executionId,
            false,
            {},
            { supported: this.supports('command_cancellation') },
          ),
        },
        true,
        entry.id,
      );
    }
  }

  private async restoreServers(
    state: ServerClientState,
    continuity: SnapshotExecutionContinuity,
    signal?: AbortSignal,
  ): Promise<void> {
    if (state.servers.length === 0) return;
    signal?.throwIfAborted();
    const inventory =
      continuity === 'disk_only'
        ? undefined
        : parseServerProcessInventory(await this.client.listProcesses(this.sandboxId));
    signal?.throwIfAborted();
    const decisions = state.servers.map((entry) => {
      const processes = inventory?.outcome === 'processes' ? inventory.processes : [];
      const found = processes.find(
        (process) =>
          process.state === 'running' &&
          (entry.params.name === undefined
            ? process.execution_id === entry.executionId
            : process.name === entry.params.name),
      );
      const exited = processes.some(
        (process) => process.execution_id === entry.executionId && process.state === 'exited',
      );
      if (continuity !== 'disk_only' && found === undefined && !exited) {
        throw new SandboxWsError(
          'unsupported',
          'server restoration needs authoritative process identity or exit evidence to avoid duplicate processes',
        );
      }
      return { entry, found };
    });
    const owned: Command[] = [];
    try {
      for (const { entry, found } of decisions) {
        signal?.throwIfAborted();
        const spec = this.restoredServerSpec(entry);
        const starting = signal === undefined ? spec : { ...spec, run: { ...spec.run, signal } };
        const launched =
          found === undefined
            ? await this.launchServer(spec, (command) => owned.push(command), signal)
            : await this.awaitServer(
                starting,
                new Command(
                  this.client,
                  this.sandboxId,
                  found.execution_id,
                  false,
                  {},
                  { supported: this.supports('command_cancellation') },
                ),
                false,
              );
        signal?.throwIfAborted();
        this.registerServer(spec, launched, true, entry.id);
      }
    } catch (error) {
      this.servers.clear();
      this.serverRegistrations.clear();
      const settled = await Promise.all(
        owned.map(async (command) => ({
          id: command.cmdId,
          stopped: await this.stopOwnedRestoredCommand(command),
        })),
      );
      attachRestoreCleanup(error, {
        stoppedExecutionIds: settled.filter(({ stopped }) => stopped).map(({ id }) => id),
        incompleteExecutionIds: settled.filter(({ stopped }) => !stopped).map(({ id }) => id),
      });
      throw error;
    }
  }

  private async stopOwnedRestoredCommand(command: Command): Promise<boolean> {
    let expired = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<boolean>((resolve) => {
      timer = setTimeout(() => {
        expired = true;
        resolve(false);
      }, RESTORE_CLEANUP_TIMEOUT_MS);
    });
    const settling = (async () => {
      try {
        await command.stop({ graceMs: 0 });
      } catch {
        if (expired) return false;
        await command.kill('kill');
      }
      let killed = false;
      while (!expired) {
        const state = await this.client.pollCommand(this.sandboxId, command.cmdId);
        if (state.outcome === 'completed') return true;
        if (state.outcome === 'not_found') return false;
        if (expired) return false;
        if (!killed) {
          await command.kill('kill');
          killed = true;
        }
        if (!expired) await sleep(RESTORE_CLEANUP_POLL_MS);
      }
      return false;
    })();
    try {
      return await Promise.race([settling, timeout]);
    } catch {
      return false;
    } finally {
      expired = true;
      clearTimeout(timer);
    }
  }

  /** Re-runs the servers whose process is gone, after the sandbox came back. Tries all, then throws the first failure. */
  private async restartGoneServers(): Promise<void> {
    let failure: { error: unknown } | undefined;
    const servers = [...this.servers];
    for (const server of servers) {
      try {
        const state = await this.client.pollCommand(this.sandboxId, server.command.cmdId);
        if (state.outcome === 'completed' || state.outcome === 'not_found') {
          await server.restart();
        } else {
          const registration = this.serverRegistrations.get(server);
          if (registration !== undefined && registration.spec.preview !== false) {
            const preview = await this.preview(registration.spec.port);
            registration.launched.url = preview.url;
            registration.launched.expiresAt = preview.expiresAt;
          }
        }
      } catch (error) {
        failure ??= { error };
      }
    }
    if (failure !== undefined) throw failure.error;
  }

  private async launchServer(
    spec: ServerSpec,
    onOwnedCommand?: (command: Command) => void,
    readinessSignal?: AbortSignal,
  ): Promise<LaunchedServer> {
    readinessSignal?.throwIfAborted();
    const { run, name } = spec;
    const params = {
      ...run,
      detached: true as const,
      deadline: 'lease' as const,
      process: name === undefined ? undefined : { name },
    };
    const command = name === undefined ? await this.runCommand(params) : await this.runNamedCommand(params);
    const ownsCommand = name === undefined || command.origin === 'created';
    if (ownsCommand) onOwnedCommand?.(command);
    const readiness = readinessSignal === undefined ? spec : { ...spec, run: { ...spec.run, signal: readinessSignal } };
    const waiting = this.awaitServer(readiness, command, ownsCommand);
    try {
      return await waiting;
    } catch (error) {
      attachExecutionIdentity(error, command.cmdId);
      throw error;
    }
  }

  private async runNamedCommand(params: RunCommandParams): Promise<Command> {
    const { signal, ...run } = params;
    signal?.throwIfAborted();
    const starting = this.runCommand({ ...run, detached: true, deadline: 'lease' });
    try {
      return signal === undefined ? await starting : await rejectOnAbort(starting, signal);
    } catch (error) {
      if (!signal?.aborted) throw error;
      const receipt = attachCommandCleanup(error, { cleanupPending: true });
      void starting
        .then(async (command) => {
          attachExecutionIdentity(error, command.cmdId);
          await this.cleanupFailedCommand(
            command,
            error,
            command.origin === 'created',
            run.cancellationTimeoutMs,
            receipt,
          );
        })
        .catch(() => {});
      throw error;
    }
  }

  private async cleanupFailedCommand(
    command: Command,
    error: unknown,
    owned: boolean,
    timeoutMs?: number,
    receipt = attachCommandCleanup(error, { executionId: command.cmdId, cleanupPending: true }),
  ): Promise<void> {
    receipt.executionId = command.cmdId;
    if (!owned) {
      receipt.cleanupPending = command.origin !== 'reused';
      return;
    }
    try {
      await cancelOwnedCommand(this.client, this.sandboxId, command.cmdId, cancellationTimeout(timeoutMs), error);
      receipt.cleanupPending = false;
    } catch {}
  }

  private async awaitServer(spec: ServerSpec, command: Command, ownsCommand = true): Promise<LaunchedServer> {
    const { run, port, path, preview, budgetMs } = spec;
    const observe = <T>(pending: Promise<T>): Promise<T> =>
      run.signal === undefined ? pending : rejectOnAbort(pending, run.signal);
    const deadline = Date.now() + budgetMs;
    const fail = async (error: unknown): Promise<never> => {
      attachExecutionIdentity(error, command.cmdId);
      await this.cleanupFailedCommand(command, error, ownsCommand, run.cancellationTimeoutMs);
      throw error;
    };
    let portOpen = false;
    try {
      for (;;) {
        if (run.signal?.aborted) {
          throw run.signal.reason;
        }
        const state = await observe(this.client.pollCommand(this.sandboxId, command.cmdId));
        run.signal?.throwIfAborted();
        if (state.outcome === 'completed') {
          const decoder = new TextDecoder();
          const output = decoder.decode(decodeBytes(state.stdout)) + decoder.decode(decodeBytes(state.stderr));
          throw new ServerExitedError(state.exit_code, output.slice(-START_SERVER_LOG_TAIL_CHARS), {
            leaseId: this.sandboxId,
          });
        }
        if (!portOpen) {
          const ready = await observe(this.client.waitSandboxReady(this.sandboxId, [port], START_SERVER_SLICE_SECONDS));
          run.signal?.throwIfAborted();
          if (ready.outcome === 'unsupported') {
            throw new SandboxWsError('unsupported', `provider ${this.provider} cannot check ports in a sandbox`, {
              leaseId: this.sandboxId,
            });
          }
          portOpen = ready.outcome === 'ready';
        }
        if (portOpen) {
          const ready = path === undefined || (await observe(this.httpAnswers(port, path)));
          run.signal?.throwIfAborted();
          if (ready) break;
        }
        if (Date.now() >= deadline) {
          const what = portOpen ? `${path} on port ${port} to answer` : `port ${port} to accept connections`;
          const logs = await this.recentOutput(command);
          throw new TimeoutError(
            `timed out after ${budgetMs} ms waiting for ${what}${logs === '' ? '' : `:\n${logs}`}`,
            {
              leaseId: this.sandboxId,
            },
          );
        }
        await sleep(START_SERVER_PAUSE_MS);
      }
    } catch (err) {
      attachExecutionIdentity(err, command.cmdId);
      // A server that already exited has nothing to stop; anything else
      // (an abort, a timeout, a transport error) must not leak the process.
      if (err instanceof ServerExitedError) throw err;
      return fail(err);
    }
    if (preview === false) {
      return { command };
    }
    try {
      const { url, expiresAt } = await observe(this.preview(port));
      run.signal?.throwIfAborted();
      return { command, url, expiresAt };
    } catch (err) {
      return fail(err);
    }
  }

  private async httpAnswers(port: number, path: string): Promise<boolean> {
    const probe = await this.runCommand({
      cmd: 'sh',
      args: ['-c', HTTP_PROBE_SCRIPT, 'probe', `http://127.0.0.1:${port}${path}`],
      timeoutSeconds: 10,
    });
    const status = Number(probe.stdout().trim());
    return Number.isInteger(status) && status > 0 && ![502, 503, 504].includes(status);
  }

  /** The last output the command wrote, or what was read before the broker refused: empty without durable logs. */
  private async recentOutput(command: Command): Promise<string> {
    const decoder = new TextDecoder();
    let text = '';
    try {
      let after: number | undefined;
      for (let page = 0; page < START_SERVER_MAX_LOG_PAGES; page += 1) {
        const got = await command.logPage({ after });
        for (const chunk of got.chunks) {
          text += decoder.decode(chunk.data, { stream: true });
        }
        text = text.slice(-START_SERVER_LOG_TAIL_CHARS);
        if (!got.hasMore) break;
        after = got.nextSeq;
      }
    } catch {
      return text;
    }
    return text;
  }

  /**
   * Freezes the sandbox in place, keeping its id and state. It is not reaped
   * while paused, up to `maxPausedMs` (server default one hour). Idempotent.
   * Resolves the time it is reaped if nobody resumes it. Throws `unsupported`
   * when the provider cannot pause.
   */
  async pause(opts: { maxPausedMs?: number } = {}): Promise<Date> {
    const response = await this.client.pauseLease(
      this.sandboxId,
      opts.maxPausedMs === undefined ? undefined : Math.ceil(opts.maxPausedMs / 1000),
    );
    if (response.outcome === 'unsupported') {
      throw new SandboxWsError('unsupported', `provider ${this.provider} cannot pause a sandbox`, {
        leaseId: this.sandboxId,
      });
    }
    return wireSystemTimeToDate(response.reap_at);
  }

  /**
   * Suspends the sandbox: its host resources are released while its id and
   * state stay. `level` is `auto` (default), `memory` or `disk`; the last two
   * must match what the provider offers. It is not reaped while suspended, up
   * to `maxSuspendedMs` (server default 24 hours). Idempotent. Resolves the
   * level it landed on and the time it is reaped if nobody wakes it. Throws a
   * `lease_denied` error of kind `suspend_unsupported` when the provider
   * cannot suspend to that level.
   */
  async suspend(
    opts: { level?: SuspendRequestLevel; maxSuspendedMs?: number } = {},
  ): Promise<{ level: SuspendLevel; reapAt: Date }> {
    const response = await this.client.suspendLease(this.sandboxId, {
      level: opts.level,
      maxSuspendedSeconds: opts.maxSuspendedMs === undefined ? undefined : Math.ceil(opts.maxSuspendedMs / 1000),
    });
    return { level: response.level, reapAt: wireSystemTimeToDate(response.reap_at) };
  }

  /**
   * Wakes a suspended sandbox with the run time it had left. Idempotent. Restarts
   * gone `startServer` servers like `resume()`.
   * Throws a `lease_denied` error of kind `lease_waking`, which carries
   * `retryAfterMs`, while another wake is in flight.
   */
  async wake(): Promise<Date> {
    const response = await this.client.wakeLease(this.sandboxId);
    this._deadline = wireSystemTimeToDate(response.deadline);
    await this.restartGoneServers();
    return this._deadline;
  }

  /**
   * Thaws a paused sandbox with the run time it had left. Idempotent. Throws `unsupported` when the provider cannot pause.
   * Servers from `startServer` whose process is gone are started again and their preview URLs refreshed; if one
   * fails to come back the error is thrown after the rest were tried, and the sandbox is already resumed.
   */
  async resume(): Promise<Date> {
    const response = await this.client.resumeLease(this.sandboxId);
    if (response.outcome === 'unsupported') {
      throw new SandboxWsError('unsupported', `provider ${this.provider} cannot resume a sandbox`, {
        leaseId: this.sandboxId,
      });
    }
    this._deadline = wireSystemTimeToDate(response.deadline);
    await this.restartGoneServers();
    return this._deadline;
  }

  /**
   * Captures a snapshot of the running sandbox without stopping it. Start a new
   * sandbox from the receipt with `Sandbox.create(client, { fromSnapshot })`, and
   * free it with `Sandbox.deleteSnapshot`. Throws `unsupported` when the provider
   * cannot checkpoint a running sandbox.
   *
   * Some providers hold the snapshot themselves and have no registry artifact.
   * Pass `acceptNative: true` to receive their {@link NativeSnapshotReceipt};
   * without it those providers throw `unsupported`. Only the same provider can
   * restore a native receipt, and deleting it is up to the provider.
   */
  snapshot(opts?: SnapshotOptions & { acceptNative?: false }): Promise<SandboxSnapshotReceipt>;
  snapshot(
    opts: SnapshotOptions & { acceptNative: true },
  ): Promise<SandboxSnapshotReceipt | SandboxNativeSnapshotReceipt>;
  snapshot(opts: SnapshotOptions): Promise<SandboxSnapshotReceipt | SandboxNativeSnapshotReceipt>;
  async snapshot(opts: SnapshotOptions = {}): Promise<SandboxSnapshotReceipt | SandboxNativeSnapshotReceipt> {
    const parsed = snapshotOptionsSchema.safeParse(opts);
    if (!parsed.success) throw new SandboxWsError('invalid_request', 'invalid snapshot options');
    if (parsed.data.expiresInMs !== undefined) {
      throw new SandboxWsError('unsupported', 'the broker does not support caller-selected snapshot expiry');
    }
    const clientState = this.exportClientState();
    const acceptNative = parsed.data.acceptNative === true;
    const response = await this.client.snapshotLease(this.sandboxId, acceptNative ? { acceptNative } : undefined);
    if (response.outcome === 'unsupported') {
      throw new SandboxWsError('unsupported', `provider ${this.provider} cannot snapshot a running sandbox`, {
        leaseId: this.sandboxId,
      });
    }
    if (response.outcome === 'created_native' && !acceptNative) {
      throw new SandboxWsError(
        'unsupported',
        `provider ${this.provider} returned a native snapshot that this call did not ask for`,
        {
          leaseId: this.sandboxId,
        },
      );
    }
    return clientState.servers.length === 0 ? response.receipt : { ...response.receipt, clientState };
  }

  async delete(): Promise<void> {
    await this.client.deleteLease(this.sandboxId);
  }

  /**
   * Scoped disposal for `await using sandbox = await Sandbox.create(...)`.
   * A named sandbox is meant to be found again by `Sandbox.get`, so it is
   * released with `stop()`; an unnamed one has no way back and is deleted.
   * Calling it again, or after the scope already disposed it, does nothing.
   * A failed release leaves the sandbox undisposed, so a later call retries.
   */
  async [Symbol.asyncDispose](): Promise<void> {
    if (this.disposal === undefined) {
      this.disposal = (this.name === undefined ? this.delete() : this.stop()).catch((err: unknown) => {
        this.disposal = undefined;
        throw err;
      });
    }
    await this.disposal;
  }

  async extendTimeout(ms: number): Promise<Date> {
    const additionalSeconds = Math.ceil(ms / 1000);
    const response = await this.client.extendLeaseTimeout(this.sandboxId, additionalSeconds);
    this._deadline = wireSystemTimeToDate(response.new_deadline);
    return this._deadline;
  }

  /**
   * Usage the broker recorded for this sandbox: commands run, bytes moved,
   * running and paused time, and cpu and memory gauges where the provider
   * reports them (`gauges` is undefined otherwise). After the sandbox has been
   * stopped or deleted the final figures are still returned until the broker
   * expires them, after which this rejects with `not_found`.
   */
  async usage(): Promise<SandboxUsage> {
    const u = await this.client.leaseUsage(this.sandboxId);
    return {
      status: u.status,
      createdAt: wireSystemTimeToDate(u.created_at),
      endedAt: u.ended_at === undefined ? undefined : wireSystemTimeToDate(u.ended_at),
      lastActivityAt: wireSystemTimeToDate(u.last_activity_at),
      commandsRun: u.commands_run,
      commandOutputBytes: u.command_output_bytes,
      fileBytesIn: u.file_bytes_in,
      fileBytesOut: u.file_bytes_out,
      runningSeconds: u.running_seconds,
      pausedSeconds: u.paused_seconds,
      suspendedSeconds: u.suspended_seconds ?? 0,
      suspendArtifactBytes: u.suspend_artifact_bytes,
      gauges:
        u.gauges === undefined
          ? undefined
          : {
              cpuNanosTotal: u.gauges.cpu_nanos_total,
              memoryBytes: u.gauges.memory_bytes,
              memoryPeakBytes: u.gauges.memory_peak_bytes,
              memoryLimitBytes: u.gauges.memory_limit_bytes,
            },
    };
  }

  /** Replaces the lease's tags. The ownership tag is kept if the lease has it and the new tags leave room. */
  async updateTags(tags: SandboxTags): Promise<void> {
    const owned = this.tags?.[OWNER_TAG_KEY] === OWNER_TAG_VALUE;
    await this.client.updateTags(this.sandboxId, withOwnershipTag(tags, owned) ?? tags);
  }

  async updateNetworkPolicy(policy: NetworkPolicyInput): Promise<void> {
    await this.client.updateNetworkPolicy(this.sandboxId, toWireNetworkPolicy(policy));
  }

  async createUser(username: string): Promise<SandboxUser> {
    return this.client.createUser(this.sandboxId, username);
  }

  asUser(username: string): SandboxUser {
    return this.client.asUser(this.sandboxId, username);
  }

  async createGroup(groupname: string): Promise<{ sharedDir: string }> {
    return this.client.createGroup(this.sandboxId, groupname);
  }

  async addUserToGroup(username: string, groupname: string): Promise<void> {
    await this.client.addUserToGroup(this.sandboxId, username, groupname);
  }

  async removeUserFromGroup(username: string, groupname: string): Promise<void> {
    await this.client.removeUserFromGroup(this.sandboxId, username, groupname);
  }
}

function isFileNotFoundError(err: unknown): boolean {
  return err instanceof Error && /no such file|file not found/i.test(err.message);
}

/** Change-tracking state shared by every `fs` view of one sandbox handle. */
interface ChangeMarks {
  /** The `trackChanges` checkpoint, used when `changes` is called without `since`. */
  baseline?: string;
  /** `mark` names to checkpoint ids. */
  marks: Map<string, string>;
}

export interface FileStat {
  size: number;
  mode: number;
  isDir: boolean;
  isFile: boolean;
  isSymlink: boolean;
  mtimeMs: number;
}

const STAT_FORMAT = '%s %f %Y';

function parseStat(stdout: string): FileStat {
  const [sizeStr, modeHexStr, mtimeStr] = stdout.trim().split(/\s+/);
  const mode = Number.parseInt(modeHexStr, 16);
  // The high nibble of `stat --format=%f` encodes the file type (see `man 2 stat`'s `S_IFMT` bits).
  const fileType = mode & 0xf000;
  return {
    size: Number.parseInt(sizeStr, 10),
    mode: mode & 0xfff,
    isDir: fileType === 0x4000,
    isFile: fileType === 0x8000,
    isSymlink: fileType === 0xa000,
    mtimeMs: Number.parseInt(mtimeStr, 10) * 1000,
  };
}

/**
 * File access, edits, search, transfers and tracking through `sandbox.fs`.
 * Structured operations use dedicated verbs; shell helpers require the
 * matching coreutils in the sandbox image.
 */
export class FileSystem {
  constructor(
    private readonly client: SandboxClientApi,
    private readonly sandboxId: LeaseId,
    private readonly tracking: ChangeMarks = { marks: new Map() },
  ) {}

  async list(path: string, opts: ListFilesOptions = {}) {
    const { user, ...input } = parseFileInput(fileListInputSchema, { ...opts, path });
    const { path: target, ...options } = input;
    return parseFileOutput(
      fileListingSchema,
      await this.client.listDir(this.sandboxId, target, {
        ...options,
        ...(user === undefined ? {} : { asUser: user }),
      }),
    );
  }

  async edit(path: string, opts: EditFileOptions) {
    const {
      find,
      replace,
      user,
      expectedOccurrences,
      path: target,
    } = parseFileInput(fileEditInputSchema, { ...opts, path });
    return parseFileOutput(
      fileEditResultSchema,
      await this.client.editFile(this.sandboxId, target, find, replace, {
        ...(expectedOccurrences === undefined ? {} : { expectedOccurrences }),
        ...(user === undefined ? {} : { asUser: user }),
      }),
    );
  }

  async search(query: string, opts: SearchFilesOptions = {}) {
    const {
      user,
      path,
      query: pattern,
      ...options
    } = parseFileInput(fileSearchInputSchema, { path: '/', ...opts, query });
    return parseFileOutput(
      fileSearchResultSchema,
      await this.client.searchFiles(this.sandboxId, path, {
        pattern,
        ...options,
        ...(user === undefined ? {} : { asUser: user }),
      }),
    );
  }

  private async run(command: string[], opts?: { user?: string }): Promise<CommandFinished> {
    const response = await this.client.runCommand(this.sandboxId, command, { runAsUser: opts?.user });
    const result = Command.fromRunCommandResponse(this.client, this.sandboxId, response);
    if (result instanceof CommandFinished) {
      return result;
    }
    return result.wait();
  }

  async stat(path: string, opts?: { user?: string }): Promise<FileStat> {
    const result = await this.run(['stat', `--format=${STAT_FORMAT}`, path], opts);
    if (result.exitCode !== 0) {
      throw new Error(`stat ${path} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
    return parseStat(result.stdout());
  }

  async ls(path: string, opts?: { user?: string }): Promise<string[]> {
    const result = await this.run(['ls', '-A', '-1', path], opts);
    if (result.exitCode !== 0) {
      throw new Error(`ls ${path} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
    return result
      .stdout()
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => line.length > 0);
  }

  /**
   * Records the tree under `path` as a baseline for `changes` and returns its id. `hash`
   * also fingerprints content so a rewrite with identical bytes is not a change. Unlike the
   * rest of `fs`, this is a dedicated wire verb and needs a provider with change tracking.
   */
  async checkpoint(path: string, opts?: { hash?: boolean }): Promise<{ checkpointId: string; entries: number }> {
    return this.client.checkpointFiles(this.sandboxId, path, opts);
  }

  /**
   * Records a named point to diff against later, the way `checkpoint` does, and returns
   * the name to pass as `changes({ since })`. Without a name the checkpoint id is the
   * name. Marking again under the same name moves it. Marks live on this handle only.
   */
  async mark(name?: string, opts?: { path?: string; hash?: boolean }): Promise<string> {
    const { checkpointId } = await this.client.checkpointFiles(this.sandboxId, opts?.path ?? '/', {
      hash: opts?.hash,
    });
    const key = name ?? checkpointId;
    this.tracking.marks.set(key, checkpointId);
    return key;
  }

  /**
   * Paths added, modified and deleted since a checkpoint or a `mark` name. Without
   * `since` it measures from the `trackChanges` baseline when the sandbox was created
   * with one, else from the lease start. The result is the net difference between two
   * scans.
   */
  async changes(opts?: { since?: string; path?: string }): Promise<FileChangeSet> {
    const since =
      opts?.since === undefined ? this.tracking.baseline : (this.tracking.marks.get(opts.since) ?? opts.since);
    return this.client.fileChanges(this.sandboxId, { ...opts, since });
  }

  /**
   * Yields create, modify and delete events under `path` until the `signal` aborts or the
   * consumer stops iterating. Needs a provider with file watch support.
   */
  watch(path: string, opts?: { recursive?: boolean; signal?: AbortSignal }): AsyncGenerator<FileWatchItem, void, void> {
    return this.client.watchFiles(this.sandboxId, path, opts);
  }

  /**
   * Uploads one file in chunks, continuing from the bytes already in the sandbox when they
   * match, then verifies a SHA-256 of the whole file. Needs a provider with directory
   * transfer support.
   */
  async uploadFile(
    path: string,
    data: Uint8Array,
    opts?: { mode?: number; signal?: AbortSignal },
  ): Promise<TransferReport> {
    return this.client.uploadFileVerified(this.sandboxId, path, data, opts);
  }

  /**
   * Downloads one file in pages and verifies its SHA-256. `partial` holds the bytes of an
   * interrupted earlier attempt to continue after.
   */
  async downloadFile(
    path: string,
    opts?: { partial?: Uint8Array; signal?: AbortSignal },
  ): Promise<{ data: Uint8Array; report: TransferReport }> {
    return this.client.downloadFileVerified(this.sandboxId, path, opts);
  }

  /**
   * Uploads one file from a stream of byte pieces, hashing as it goes so the whole file is
   * never buffered, then verifies the SHA-256 against the sandbox copy. Starts at byte 0
   * unless `resume` is set: then `source` must replay the same bytes from the start, the
   * bytes already in the sandbox are skipped once their digest matches, and a copy that is
   * not a prefix of `source` throws. Needs a provider with directory transfer support.
   */
  async uploadFileStream(
    path: string,
    source: Iterable<Uint8Array> | AsyncIterable<Uint8Array>,
    opts?: { mode?: number; signal?: AbortSignal; resume?: boolean },
  ): Promise<TransferReport> {
    return this.client.uploadFileStream(this.sandboxId, path, source, opts);
  }

  /**
   * Downloads one file page by page without buffering it. By default the SHA-256 is checked
   * only after the last page and a mismatch throws from the iterator, so treat what was
   * consumed as unverified until iteration ends cleanly. `verifyPages` also checks each page
   * against a ranged digest before yielding it, at one extra round trip per page. Returns the
   * transfer report.
   */
  downloadFileStream(
    path: string,
    opts?: { signal?: AbortSignal; verifyPages?: boolean },
  ): AsyncGenerator<Uint8Array, TransferReport, void> {
    return this.client.downloadFileStream(this.sandboxId, path, opts);
  }

  /**
   * Copies `files` into the sandbox directory `dir`, one resumable, verified file at a time,
   * with no archive size ceiling. Paths are relative to `dir` and cannot climb out of it.
   */
  async uploadDir(
    dir: string,
    files: Iterable<DirFile> | AsyncIterable<DirFile>,
    opts?: { emptyDirs?: string[]; signal?: AbortSignal },
  ): Promise<TransferReport> {
    return this.client.uploadDir(this.sandboxId, dir, files, opts);
  }

  /**
   * Reads every regular file under `dir`, each verified by digest. Symlinks are skipped.
   * `signal` stops it between pages.
   */
  async downloadDir(dir: string, opts?: { signal?: AbortSignal }): Promise<DownloadedDir> {
    return this.client.downloadDir(this.sandboxId, dir, opts);
  }

  async exists(path: string, opts?: { user?: string }): Promise<boolean> {
    const result = await this.run(['test', '-e', path], opts);
    return result.exitCode === 0;
  }

  async rm(path: string, opts?: { recursive?: boolean; user?: string }): Promise<void> {
    const args = opts?.recursive ? ['-rf', path] : ['-f', path];
    const result = await this.run(['rm', ...args], opts);
    if (result.exitCode !== 0) {
      throw new Error(`rm ${path} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
  }

  async mkdir(path: string, opts?: { user?: string }): Promise<void> {
    const result = await this.run(['mkdir', '-p', path], opts);
    if (result.exitCode !== 0) {
      throw new Error(`mkdir ${path} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
  }

  async cp(src: string, dst: string, opts?: { user?: string }): Promise<void> {
    const result = await this.run(['cp', '-r', src, dst], opts);
    if (result.exitCode !== 0) {
      throw new Error(`cp ${src} ${dst} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
  }

  async mv(src: string, dst: string, opts?: { user?: string }): Promise<void> {
    const result = await this.run(['mv', src, dst], opts);
    if (result.exitCode !== 0) {
      throw new Error(`mv ${src} ${dst} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
  }

  async chmod(path: string, mode: number, opts?: { user?: string }): Promise<void> {
    const octal = (mode & 0xfff).toString(8);
    const result = await this.run(['chmod', octal, path], opts);
    if (result.exitCode !== 0) {
      throw new Error(`chmod ${octal} ${path} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
  }

  async readlink(path: string, opts?: { user?: string }): Promise<string> {
    const result = await this.run(['readlink', '-f', path], opts);
    if (result.exitCode !== 0) {
      throw new Error(`readlink ${path} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
    return result.stdout().trim();
  }

  async symlink(target: string, path: string, opts?: { user?: string }): Promise<void> {
    const result = await this.run(['ln', '-s', target, path], opts);
    if (result.exitCode !== 0) {
      throw new Error(`symlink ${target} -> ${path} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
  }
}

export class Drive {
  readonly name: string;
  readonly tenant: TenantId;
  readonly sizeGib: number;
  /** Set for an S3-backed drive. */
  readonly bucket?: string;
  /** Set for a drive backed by a provider-native volume. */
  readonly providerVolume?: ProviderVolumeRef;
  readonly createdAt: Date;
  readonly attachedLease?: LeaseId;

  constructor(
    private readonly client: SandboxClientApi,
    record: DriveRecord,
  ) {
    this.name = record.id.name;
    this.tenant = record.tenant;
    this.sizeGib = record.size_gib;
    this.bucket = record.bucket;
    this.providerVolume = record.provider_volume;
    this.createdAt = wireSystemTimeToDate(record.created_at);
    this.attachedLease = record.attached_lease;
  }

  static async list(client: SandboxClientApi): Promise<Drive[]> {
    const records = await client.listDrives();
    return records.map((record) => new Drive(client, record));
  }

  static async create(
    client: SandboxClientApi,
    opts: { name: string; sizeGib?: number } & (
      | { bucket: string; providerVolume?: never }
      | { providerVolume: ProviderVolumeRef; bucket?: never }
    ),
  ): Promise<Drive> {
    const record = await client.createDrive(opts.name, opts.bucket ?? opts.providerVolume, { sizeGib: opts.sizeGib });
    return new Drive(client, record);
  }

  async delete(): Promise<void> {
    await this.client.deleteDrive(this.name);
  }
}

export interface TerminalOptions {
  /** Terminal width in columns. Defaults to 80. */
  cols?: number;
  /** Terminal height in rows. Defaults to 24. */
  rows?: number;
  /** Program and arguments. Omit for the sandbox's default shell. */
  cmd?: string[];
  env?: Record<string, string>;
  cwd?: string;
}

/**
 * What a terminal produced: raw bytes, its end with the exit code when known, or a `resync`
 * after a reattach that lost output (discard the screen state; output continues from `fromSeq`).
 */
export type TerminalChunk =
  | { type: 'data'; seq: number; bytes: Uint8Array }
  | { type: 'exit'; seq: number; exitCode?: number }
  | { type: 'resync'; fromSeq: number };

function base64ToBytes(encoded: string): Uint8Array {
  const binary = atob(encoded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

/**
 * An open interactive terminal. Iterate it for output; a `seq` gap means a
 * slow reader lost frames. The stream ends after the `exit` chunk.
 */
export class Terminal implements AsyncIterable<TerminalChunk> {
  private closing: Promise<void> | undefined;

  constructor(
    private readonly client: SandboxClientApi,
    readonly terminalId: string,
    private readonly events: AsyncIterable<TerminalOutputEvent>,
  ) {}

  async write(data: string | Uint8Array): Promise<void> {
    const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
    for (let offset = 0; offset < bytes.length; offset += MAX_TERMINAL_INPUT_BYTES) {
      await this.client.terminalInput(this.terminalId, bytes.subarray(offset, offset + MAX_TERMINAL_INPUT_BYTES));
    }
  }

  async resize(cols: number, rows: number): Promise<void> {
    await this.client.terminalResize(this.terminalId, cols, rows);
  }

  /** Ends the terminal. Safe to call more than once. */
  close(): Promise<void> {
    this.closing ??= this.client.terminalClose(this.terminalId);
    return this.closing;
  }

  async *[Symbol.asyncIterator](): AsyncIterator<TerminalChunk> {
    for await (const event of this.events) {
      if (event.type === 'data') {
        yield { type: 'data', seq: event.seq, bytes: base64ToBytes(event.data) };
      } else if (event.type === 'resync') {
        yield { type: 'resync', fromSeq: event.from_seq };
      } else {
        yield event.exit_code === undefined
          ? { type: 'exit', seq: event.seq }
          : { type: 'exit', seq: event.seq, exitCode: event.exit_code };
      }
    }
  }
}

/**
 * Capability names used by other sandbox SDKs, mapped to this client's feature names,
 * so code written against them reads the same here.
 */
export const CAPABILITY_ALIASES = {
  pause: 'pause_resume',
  resume: 'pause_resume',
  snapshot: 'snapshots',
  stdin: 'write_stdin',
  logs: 'read_logs',
  watch: 'file_watch',
  changes: 'change_tracking',
  domain: 'sandbox_domain',
} as const satisfies Record<string, SandboxFeature>;

export type CapabilityName = SandboxFeature | keyof typeof CAPABILITY_ALIASES;

/** The feature a capability name or alias stands for. */
export type FeatureOf<N extends CapabilityName> = N extends keyof typeof CAPABILITY_ALIASES
  ? (typeof CAPABILITY_ALIASES)[N]
  : N;

/**
 * True when the sandbox's provider is known to support `capability`, narrowing the
 * handle like `sandbox.supports`. Accepts this client's feature names and the common
 * aliases in `CAPABILITY_ALIASES` (`pause`, `snapshot`, ...). False when the
 * capabilities are unknown.
 */
export function hasCapability<N extends CapabilityName>(
  sandbox: Sandbox,
  capability: N,
): sandbox is SandboxWith<FeatureOf<N>> {
  const feature = (
    capability in CAPABILITY_ALIASES ? CAPABILITY_ALIASES[capability as keyof typeof CAPABILITY_ALIASES] : capability
  ) as FeatureOf<N>;
  return sandbox.supports(feature);
}

/**
 * Lists the sandboxes this SDK created, the same as `Sandbox.list`. `labels` is the
 * other SDKs' name for tags: only sandboxes carrying every label are returned.
 */
export async function listSandboxes(
  client: SandboxClientApi,
  opts?: { labels?: SandboxTags; includeForeign?: boolean },
): Promise<Sandbox[]> {
  return Sandbox.list(client, { tags: opts?.labels, includeForeign: opts?.includeForeign });
}

/**
 * Releases this SDK's sandboxes matching every filter, the same as `Sandbox.cleanup`
 * with `labels` for `tags`.
 */
export async function reapSandboxes(
  client: SandboxClientApi,
  opts: Omit<NonNullable<Parameters<typeof Sandbox.cleanup>[1]>, 'tags'> & { labels?: SandboxTags } = {},
): Promise<ReapLeasesResponse> {
  const { labels, ...rest } = opts;
  return Sandbox.cleanup(client, { ...rest, tags: labels });
}

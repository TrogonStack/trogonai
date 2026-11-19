import { z } from 'zod';

import { cancellationRunResponse } from './command-cancellation.ts';
import {
  parseCancelCommandLaunchRequest,
  parseCancelCommandLaunchResponse,
  requireCommandLaunchCancellation,
  retainCommandLaunchAuthority,
} from './command-launch.ts';
import type { CancelCommandLaunchResponse } from './command-launch.ts';
import { errorFromCode, SandboxWsError, TimeoutError, TransportError } from './errors.ts';
import {
  base64ToBytes,
  bytesToBase64,
  EditRejectedError,
  FILE_OP_MAX_ENTRIES,
  FILE_TRANSFER_CHUNK_BYTES,
  FileNotFoundError,
  FileOpFailedError,
  parseFileOpResponse,
  throwOnRefusal,
} from './fileops.ts';
import type { Digest, ExecutionId, LeaseId, StdinSeq } from './ids.ts';
import { parseNativeSnapshotDeleteResponse, parseNativeSnapshotReference } from './native-snapshots.ts';
import type {
  CancelOperationOptions,
  CancelOperationResponse,
  GetOperationResponse,
  OperationCapabilities,
} from './operations.ts';
import {
  cancelOperationResponseSchema,
  getOperationResponseSchema,
  operationCapabilitiesSchema,
  parseCancelOperationOptions,
  parseOperationId,
  parseOperationResponse,
} from './operations.ts';
import type { CreateSessionResponse, WsFrame } from './protocol.ts';
import { redactValuesSchema } from './redact.ts';
import { resolveRetryPolicy, withRetry } from './retry.ts';
import type { ResolvedRetryPolicy, RetryOptions } from './retry.ts';
import { Sha256 } from './sha256.ts';
import {
  absorb,
  checkRelative,
  emptyReport,
  integrityError,
  resumeMismatchError,
  joinRemote,
  sha256Hex,
} from './transfer.ts';
import type { DirFile, DownloadedDir, TransferReport } from './transfer.ts';
import { openWebSocket } from './transport.ts';
import { SandboxUser } from './user.ts';
import { MAX_TERMINAL_INPUT_BYTES } from './wire.ts';
import type {
  CapabilitiesResponse,
  CommandDeadline,
  CreateDriveResponse,
  CreateSandboxGroupResponse,
  CreateSandboxUserResponse,
  DeleteDriveResponse,
  DeleteEntityResponse,
  DeleteNamedSandboxResponse,
  EntityRef,
  GetEntityResponse,
  ListEntitiesOptions,
  ListEntitiesResponse,
  DeleteLeaseResponse,
  DeleteSnapshotTagResponse,
  DriveRecord,
  ProviderVolumeRef,
  ExtendLeaseTimeoutResponse,
  GetLeaseByNameResponse,
  GetSandboxDomainResponse,
  NativeHandleResponse,
  NativeSnapshotRef,
  NativeSnapshotDeleteResponse,
  SshAccessResponse,
  GroupMembershipAction,
  KillCommandResponse,
  ListProcessesResponse,
  LogFollowEvent,
  ReadLogsResponse,
  RestartCommandResponse,
  StopCommandResponse,
  ProcessSpec,
  WaitReadyResponse,
  WaitSandboxReadyResponse,
  LeaseDashboardEvent,
  LeaseGranted,
  LeaseListRow,
  ReapLeasesOptions,
  ReapLeasesResponse,
  LeaseReleasedReason,
  LeaseRequest,
  ListDrivesResponse,
  NetworkPolicy,
  NetworkPolicyUpdateResponse,
  OutputRetention,
  PollCommandResponse,
  ReadFileResponse,
  RunCommandResponse,
  SandboxFileWrite,
  SandboxTags,
  SetSnapshotTagResponse,
  SnapshotDeleteResponse,
  SnapshotGetResponse,
  SnapshotLeaseResponse,
  SnapshotListRow,
  SnapshotPublishedEvent,
  SnapshotTagHistoryResponse,
  SnapshotTagRow,
  SnapshotTreeResponse,
  StdinMode,
  PauseLeaseResponse,
  ResumeLeaseResponse,
  SuspendLeaseResponse,
  SuspendRequestLevel,
  WakeLeaseResponse,
  LeaseUsage,
  TenantUsage,
  AccountLimits,
  StopLeaseResponse,
  UnixSignal,
  UpdateTagsResponse,
  WorkerHeartbeatEvent,
  WriteFilesResponse,
  WriteStdinResponse,
  FileOp,
  FileOpResponse,
  WireFileStat,
  WireFileWatchEvent,
  WireSearchMatch,
  TerminalOpenRequest,
  TerminalOutputEvent,
} from './wire.ts';

/** How long each watch poll parks at the worker; the worker caps it at 20 seconds. */
const FILE_WATCH_POLL_WAIT_MS = 15_000;

/** Net difference between a baseline and now, from `fileChanges`. */
export interface FileChangeSet {
  /** The baseline compared against: a checkpoint id or `start`. */
  since: string;
  added: string[];
  modified: string[];
  deleted: string[];
  /** True when the lists were cut at the worker's entry ceiling. */
  truncated: boolean;
}

/** One item of `watchFiles`. */
export type FileWatchItem = { type: 'event'; event: WireFileWatchEvent } | { type: 'overflowed' };

const SANDBOX_NAME_PATTERN = /^[a-z_][a-z0-9_-]*$/;
const SANDBOX_NAME_MAX_LENGTH = 32;

function assertValidSandboxName(name: string, kind: 'username' | 'groupname'): void {
  if (name.length === 0 || name.length > SANDBOX_NAME_MAX_LENGTH || !SANDBOX_NAME_PATTERN.test(name)) {
    throw new TransportError(
      `invalid ${kind} "${name}": must match /^[a-z_][a-z0-9_-]*$/ and be at most ${SANDBOX_NAME_MAX_LENGTH} characters`,
    );
  }
}

export interface SandboxWsClientOptions {
  gatewayHttpUrl: string;
  gatewayWsUrl: string;
  apiKey: string;
  /** Retry policy for idempotent requests; `false` disables it. See `RetryOptions`. */
  retry?: RetryOptions | false;
  /**
   * Default deadline for each request, in milliseconds. A request with no
   * reply by then rejects with `TimeoutError`; it may still have been
   * applied. A command's own `timeoutSeconds` and a long poll's wait are
   * added on top, so they are not cut short. Defaults to
   * `DEFAULT_REQUEST_TIMEOUT_MS` (10 s), matching the Rust client.
   */
  requestTimeoutMs?: number;
}

export interface SandboxWsTokenOptions {
  gatewayWsUrl: string;
  /** The `token` field of a `POST /v1/sessions` reply, minted by a trusted backend. */
  token: string;
  /** Retry policy for idempotent requests; `false` disables it. See `RetryOptions`. */
  retry?: RetryOptions | false;
  /**
   * Default deadline for each request, in milliseconds. A request with no
   * reply by then rejects with `TimeoutError`; it may still have been
   * applied. A command's own `timeoutSeconds` and a long poll's wait are
   * added on top, so they are not cut short. Defaults to
   * `DEFAULT_REQUEST_TIMEOUT_MS` (10 s), matching the Rust client.
   */
  requestTimeoutMs?: number;
}

const tokenOptionsSchema = z.object({
  gatewayWsUrl: z.string().regex(/^wss?:\/\//, 'must start with ws:// or wss://'),
  token: z.string().min(1, 'must not be empty'),
});

export interface EvidenceEvent {
  subjectSuffix: string;
  payload: string | Uint8Array;
}

export interface EvidenceSubscription {
  ready: Promise<void>;
  events: AsyncIterable<EvidenceEvent>;
  close: () => void;
}

export type DashboardEvent =
  | ({ stream: 'leases' } & LeaseDashboardEvent)
  | ({ stream: 'snapshots' } & SnapshotPublishedEvent)
  | ({ stream: 'workers' } & WorkerHeartbeatEvent);

const DASHBOARD_STREAM_IDS = ['sandbox.leases', 'sandbox.snapshots', 'sandbox.workers'] as const;

export async function createSession(gatewayHttpUrl: string, apiKey: string): Promise<CreateSessionResponse> {
  let response: Response;
  try {
    response = await fetch(`${gatewayHttpUrl}/v1/sessions`, {
      method: 'POST',
      headers: { 'X-Sandbox-API-Key': apiKey },
    });
  } catch (err) {
    // fetch errors can echo the URL, which may carry credentials.
    throw new TransportError(`session creation failed: ${err instanceof Error ? err.message : String(err)}`, {
      cause: err,
    });
  }
  if (response.status !== 201) {
    let message = `session creation failed with status ${response.status}`;
    try {
      const body = (await response.json()) as { error?: string };
      if (body.error) {
        message = body.error;
      }
    } catch {
      // response body was not JSON, fall back to the status-based message
    }
    throw new TransportError(message);
  }
  return (await response.json()) as CreateSessionResponse;
}

function checkRequestTimeout(ms: number | undefined): void {
  if (ms !== undefined && (!Number.isFinite(ms) || ms <= 0)) {
    throw new RangeError('requestTimeoutMs must be a positive number');
  }
}

/** The client deadline when `requestTimeoutMs` is unset, matching the Rust client. */
export const DEFAULT_REQUEST_TIMEOUT_MS = 10_000;

/**
 * Seconds added to the default deadline when a request leaves
 * `timeout_seconds` unset: the gateway's own waits for these tools. The
 * provisioning and capture verbs are bounded by the broker client's 30 s slow
 * verb deadline, which a cold provider can use in full.
 */
const SERVER_DEFAULT_WAIT_SECONDS: Readonly<Record<string, number>> = {
  sandbox_run_command: 60,
  sandbox_wait_ready: 30,
  sandbox_wait_sandbox_ready: 30,
  sandbox_lease: 30,
  sandbox_wait_released: 300,
  sandbox_fork: 30,
  sandbox_pause_lease: 30,
  sandbox_resume_lease: 30,
  sandbox_suspend_lease: 30,
  sandbox_wake_lease: 30,
  sandbox_snapshot_lease: 30,
  sandbox_snapshot_delete: 30,
  sandbox_native_snapshot_delete: 30,
  sandbox_cancel_operation: 30,
  sandbox_delete_named_sandbox: 20,
  // A terminal opens a shell on the worker, which a cold provider may still
  // be booting, so it gets a minute in total.
  sandbox_terminal_open: 50,
  // Only the subscription acknowledgement is bounded; the events that follow
  // are a stream with no deadline. The gateway attaches a JetStream consumer
  // before it acknowledges, which can be slow while the broker is busy.
  sandbox_subscribe_evidence: 20,
  sandbox_subscribe_dashboard: 20,
};

/**
 * The client deadline for one request: the configured default plus whatever
 * the request itself asks the server to wait, so a long command or long poll
 * is not cut off by the default.
 */
function requestDeadlineMs(defaultMs: number, tool: string, payload: unknown): number {
  const fields = typeof payload === 'object' && payload !== null ? (payload as Record<string, unknown>) : {};
  const op = typeof fields.op === 'object' && fields.op !== null ? (fields.op as Record<string, unknown>) : {};
  const seconds =
    typeof fields.timeout_seconds === 'number' ? fields.timeout_seconds : (SERVER_DEFAULT_WAIT_SECONDS[tool] ?? 0);
  const waitMs =
    (typeof fields.wait_ms === 'number' ? fields.wait_ms : 0) + (typeof op.wait_ms === 'number' ? op.wait_ms : 0);
  return defaultMs + seconds * 1000 + waitMs;
}

/** The gateway takes `entity_id` or `name`, never both. */
function entityPayload(entity: EntityRef): Record<string, string> {
  if ('id' in entity) {
    return { entity_id: entity.id };
  }
  return { name: entity.name };
}

function leaseIdOf(payload: unknown): string | undefined {
  if (typeof payload === 'object' && payload !== null) {
    const value = (payload as Record<string, unknown>).lease_id;
    return typeof value === 'string' ? value : undefined;
  }
  return undefined;
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
  tool: string;
  leaseId?: string;
}

interface EventQueue {
  push: (event: unknown) => void;
  end: () => void;
}

const MAX_UNCLAIMED_TERMINALS = 16;
const MAX_UNCLAIMED_TERMINAL_EVENTS = 256;

/** A terminal the gateway opened: its id and the stream of its output. */
export interface OpenedTerminal {
  terminalId: string;
  events: AsyncIterable<TerminalOutputEvent>;
}

const processSpecSchema = z.object({
  name: z
    .string()
    .regex(/^[A-Za-z0-9._-]{1,64}$/, 'process name must match [A-Za-z0-9._-]{1,64}')
    .optional(),
  readiness: z
    .discriminatedUnion('kind', [
      z.object({
        kind: z.literal('tcp_port'),
        port: z.number().int().min(1).max(65535),
      }),
      z.object({
        kind: z.literal('output_contains'),
        needle: z.string().min(1),
      }),
    ])
    .optional(),
});

function parseProcessSpec(spec: ProcessSpec): ProcessSpec {
  return processSpecSchema.parse(spec);
}

export class SandboxWsClient {
  private readonly socket: WebSocket;
  private readonly pending = new Map<string, PendingRequest>();
  private readonly eventQueues = new Map<string, EventQueue>();
  private readonly evidenceListeners = new Map<string, Set<EventQueue>>();
  private readonly evidenceSubscriptions = new Map<string, Promise<void>>();
  /**
   * Terminal output that arrived before `openTerminal` learned the terminal
   * id (the gateway subscribes before it opens, so the first prompt can beat
   * the reply). Bounded so unclaimed streams cannot grow without limit.
   */
  private readonly unclaimedTerminalEvents = new Map<string, unknown[]>();
  private closed = false;
  private readonly retryPolicy: ResolvedRetryPolicy;
  private readonly requestTimeoutMs: number;

  private constructor(socket: WebSocket, retry?: RetryOptions | false, requestTimeoutMs?: number) {
    this.socket = socket;
    this.retryPolicy = resolveRetryPolicy(retry);
    this.requestTimeoutMs = requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
    this.socket.addEventListener('message', (event: MessageEvent) => {
      this.handleMessage(event.data);
    });
    this.socket.addEventListener('close', () => {
      this.handleClose();
    });
  }

  static async connect(opts: SandboxWsClientOptions): Promise<SandboxWsClient> {
    checkRequestTimeout(opts.requestTimeoutMs);
    const session = await createSession(opts.gatewayHttpUrl, opts.apiKey);
    const socket = await SandboxWsClient.openAuthenticatedSocket(opts.gatewayWsUrl, session.token);
    return new SandboxWsClient(socket, opts.retry, opts.requestTimeoutMs);
  }

  /**
   * Opens the socket with a session token minted elsewhere, skipping
   * `POST /v1/sessions`. This is the browser path: the app's backend calls
   * the gateway session endpoint with the tenant API key and hands only the
   * short-lived token to the page, so the API key never ships to a client.
   * Throws `TransportError` if the options are malformed or the gateway
   * rejects the token.
   */
  static async connectWithToken(opts: SandboxWsTokenOptions): Promise<SandboxWsClient> {
    const parsed = tokenOptionsSchema.safeParse(opts);
    if (!parsed.success) {
      const detail = parsed.error.issues.map((issue) => `${issue.path.join('.')}: ${issue.message}`).join('; ');
      throw new TransportError(`invalid connectWithToken options: ${detail}`);
    }
    checkRequestTimeout(opts.requestTimeoutMs);
    const socket = await SandboxWsClient.openAuthenticatedSocket(parsed.data.gatewayWsUrl, parsed.data.token);
    return new SandboxWsClient(socket, opts.retry, opts.requestTimeoutMs);
  }

  private static openAuthenticatedSocket(gatewayWsUrl: string, token: string): Promise<WebSocket> {
    return new Promise((resolve, reject) => {
      const socket = openWebSocket(`${gatewayWsUrl}/v1/ws`, [`bearer.${token}`]);
      const onOpen = () => {
        cleanup();
        resolve(socket);
      };
      const onError = () => {
        cleanup();
        reject(new TransportError('authentication failed during websocket handshake'));
      };
      const onClose = () => {
        cleanup();
        reject(new TransportError('authentication failed during websocket handshake'));
      };
      const cleanup = () => {
        socket.removeEventListener('open', onOpen);
        socket.removeEventListener('error', onError);
        socket.removeEventListener('close', onClose);
      };
      socket.addEventListener('open', onOpen);
      socket.addEventListener('error', onError);
      socket.addEventListener('close', onClose);
    });
  }

  private handleMessage(data: unknown): void {
    let frame: WsFrame;
    try {
      frame = JSON.parse(String(data)) as WsFrame;
    } catch {
      return;
    }
    if (frame.kind === 'response') {
      const pending = this.pending.get(frame.id);
      if (!pending) {
        return;
      }
      this.pending.delete(frame.id);
      if (frame.error) {
        const { code, message, ...extras } = frame.error;
        pending.reject(
          errorFromCode(code, message, extras, {
            tool: pending.tool,
            leaseId: pending.leaseId,
          }),
        );
      } else {
        pending.resolve(frame.result);
      }
      return;
    }
    if (frame.kind === 'event') {
      const listeners = this.evidenceListeners.get(frame.stream_id);
      if (listeners) {
        for (const listener of listeners) {
          listener.push(frame.event);
        }
        return;
      }
      const queue = this.eventQueues.get(frame.stream_id);
      if (queue) {
        queue.push(frame.event);
        return;
      }
      this.stashUnclaimedTerminalEvent(frame.stream_id, frame.event);
    }
  }

  private stashUnclaimedTerminalEvent(streamId: string, event: unknown): void {
    const type = (event as { type?: unknown } | null)?.type;
    if (type !== 'data' && type !== 'exit' && type !== 'resync') {
      return;
    }
    let held = this.unclaimedTerminalEvents.get(streamId);
    if (!held) {
      if (this.unclaimedTerminalEvents.size >= MAX_UNCLAIMED_TERMINALS) {
        return;
      }
      held = [];
      this.unclaimedTerminalEvents.set(streamId, held);
    }
    if (held.length < MAX_UNCLAIMED_TERMINAL_EVENTS) {
      held.push(event);
    }
  }

  private handleClose(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    for (const pending of this.pending.values()) {
      pending.reject(
        new TransportError('websocket connection closed', {
          tool: pending.tool,
          leaseId: pending.leaseId,
        }),
      );
    }
    this.pending.clear();
    for (const queue of this.eventQueues.values()) {
      queue.end();
    }
    this.eventQueues.clear();
    this.unclaimedTerminalEvents.clear();
    for (const listeners of this.evidenceListeners.values()) {
      for (const listener of listeners) {
        listener.end();
      }
    }
    this.evidenceListeners.clear();
    this.evidenceSubscriptions.clear();
  }

  private dispatch<T>(tool: string, payload: unknown, idempotent?: boolean): Promise<T> {
    return withRetry(
      tool,
      this.retryPolicy,
      () => !this.closed,
      () => this.dispatchOnce<T>(tool, payload),
      idempotent,
    );
  }

  private dispatchOnce<T>(tool: string, payload: unknown): Promise<T> {
    if (this.closed) {
      return Promise.reject(
        new TransportError('client is closed', {
          tool,
          leaseId: leaseIdOf(payload),
        }),
      );
    }
    const id = crypto.randomUUID();
    const frame: WsFrame = { kind: 'request', id, tool, payload };
    return new Promise<T>((resolve, reject) => {
      const leaseId = leaseIdOf(payload);
      const deadlineMs = requestDeadlineMs(this.requestTimeoutMs, tool, payload);
      const timer = setTimeout(() => {
        if (!this.pending.delete(id)) return;
        reject(
          new TimeoutError(`no reply to ${tool} within ${deadlineMs} ms; it may still have been applied`, {
            tool,
            leaseId,
          }),
        );
      }, deadlineMs);
      this.pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          (resolve as (value: unknown) => void)(value);
        },
        reject: (reason) => {
          clearTimeout(timer);
          reject(reason);
        },
        tool,
        leaseId,
      });
      try {
        this.socket.send(JSON.stringify(frame));
      } catch (err) {
        this.pending.delete(id);
        clearTimeout(timer);
        reject(
          new TransportError(err instanceof Error ? err.message : String(err), {
            tool,
            leaseId,
            cause: err,
          }),
        );
      }
    });
  }

  async lease(request: LeaseRequest, opts?: { timeoutSeconds?: number }): Promise<LeaseGranted> {
    const payload: Record<string, unknown> = { ...request };
    if (opts?.timeoutSeconds !== undefined) {
      payload.timeout_seconds = opts.timeoutSeconds;
    }
    return this.dispatch<LeaseGranted>('sandbox_lease', payload);
  }

  async cancel(leaseId: LeaseId): Promise<void> {
    await this.dispatch<{ cancelled: true }>('sandbox_cancel', {
      lease_id: leaseId,
    });
  }

  async waitReleased(leaseId: LeaseId, opts?: { timeoutSeconds?: number }): Promise<LeaseReleasedReason> {
    const payload: Record<string, unknown> = { lease_id: leaseId };
    if (opts?.timeoutSeconds !== undefined) {
      payload.timeout_seconds = opts.timeoutSeconds;
    }
    const result = await this.dispatch<{ reason: LeaseReleasedReason }>('sandbox_wait_released', payload);
    return result.reason;
  }

  async runCommand(
    leaseId: LeaseId,
    command: string[],
    opts?: {
      cwd?: string;
      env?: Record<string, string>;
      timeoutSeconds?: number;
      detached?: boolean;
      streamOutput?: boolean;
      runAsUser?: string;
      stdin?: StdinMode;
      outputRetention?: OutputRetention;
      deadline?: CommandDeadline;
      maxOutputBytes?: number;
      process?: ProcessSpec;
      /** Exact literals to scrub from the output. See `RunCommandRequest.redact_values` for limits. */
      redactValues?: string[];
      /**
       * Makes a retried launch replay the first launch's reply instead of
       * running the command again: 1 to 128 characters of `[A-Za-z0-9_-]`,
       * unique per lease. A random key is generated when omitted, which is
       * what lets this call be retried after a dropped reply.
       */
      launchKey?: string;
      requireLaunchCancellation?: boolean;
    },
  ): Promise<RunCommandResponse> {
    const requireLaunchCancellation = requireCommandLaunchCancellation(opts?.requireLaunchCancellation, opts?.detached);
    const redact = redactValuesSchema.safeParse(opts?.redactValues ?? []);
    if (!redact.success) {
      throw new SandboxWsError('invalid_request', 'redactValues: too many values or a value is too long');
    }
    const launchKey = opts?.launchKey ?? crypto.randomUUID();
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(launchKey)) {
      throw new SandboxWsError('invalid_request', 'launchKey: must be 1 to 128 characters of [A-Za-z0-9_-]');
    }
    const pending = this.dispatch<RunCommandResponse>(
      'sandbox_run_command',
      {
        lease_id: leaseId,
        command,
        cwd: opts?.cwd,
        env: opts?.env,
        timeout_seconds: opts?.timeoutSeconds,
        detached: opts?.detached,
        stream_output: opts?.streamOutput,
        run_as_user: opts?.runAsUser,
        stdin: opts?.stdin,
        output_retention: opts?.outputRetention,
        deadline: opts?.deadline,
        max_output_bytes: opts?.maxOutputBytes,
        process: opts?.process === undefined ? undefined : parseProcessSpec(opts.process),
        redact_values: opts?.redactValues,
        launch_key: launchKey,
        require_launch_cancellation: requireLaunchCancellation || undefined,
      },
      true,
    );
    try {
      const response = await pending;
      cancellationRunResponse(response);
      return response;
    } catch (error) {
      throw requireLaunchCancellation ? retainCommandLaunchAuthority(error, leaseId, launchKey) : error;
    }
  }

  async cancelCommandLaunch(leaseId: LeaseId, launchKey: string): Promise<CancelCommandLaunchResponse> {
    const request = parseCancelCommandLaunchRequest({ lease_id: leaseId, launch_key: launchKey });
    return parseCancelCommandLaunchResponse(await this.dispatch('sandbox_cancel_command_launch', request, true));
  }

  async listProcesses(leaseId: LeaseId): Promise<ListProcessesResponse> {
    return this.dispatch<ListProcessesResponse>('sandbox_list_processes', {
      lease_id: leaseId,
    });
  }

  async waitReady(leaseId: LeaseId, executionId: string, timeoutSeconds?: number): Promise<WaitReadyResponse> {
    return this.dispatch<WaitReadyResponse>('sandbox_wait_ready', {
      lease_id: leaseId,
      execution_id: executionId,
      timeout_seconds: timeoutSeconds,
    });
  }

  /**
   * Blocks on the broker until the lease is running and every port accepts a
   * connection inside the sandbox, or `timeoutSeconds` (default 30, capped at
   * 300) elapse. `unsupported` when `ports` is set and the provider cannot exec.
   */
  async waitSandboxReady(
    leaseId: LeaseId,
    ports?: number[],
    timeoutSeconds?: number,
  ): Promise<WaitSandboxReadyResponse> {
    return this.dispatch<WaitSandboxReadyResponse>('sandbox_wait_sandbox_ready', {
      lease_id: leaseId,
      ports,
      timeout_seconds: timeoutSeconds,
    });
  }

  /** Signals the running process with this name; `not_found` when none runs under it. */
  async killProcess(leaseId: LeaseId, name: string, signal?: UnixSignal): Promise<KillCommandResponse> {
    return this.dispatch<KillCommandResponse>('sandbox_kill_process', {
      lease_id: leaseId,
      name,
      signal,
    });
  }

  async writeStdin(
    leaseId: LeaseId,
    executionId: ExecutionId,
    seq: StdinSeq,
    data: Uint8Array,
    eof: boolean,
  ): Promise<WriteStdinResponse> {
    return this.dispatch<WriteStdinResponse>('sandbox_write_stdin', {
      lease_id: leaseId,
      execution_id: executionId,
      seq,
      data: Array.from(data),
      eof,
    });
  }

  /**
   * Opens an interactive PTY terminal. Output arrives on `events` until an
   * `exit` event ends the stream. Throws a typed error when the provider has
   * no PTY support (`unsupported`) or the terminal cannot start.
   */
  async openTerminal(request: TerminalOpenRequest): Promise<OpenedTerminal> {
    const result = await this.dispatch<{ outcome: 'opened'; terminal_id: string }>('sandbox_terminal_open', {
      ...request,
    });
    const terminalId = result.terminal_id;
    const queue = this.createQueue<TerminalOutputEvent>(terminalId);
    const held = this.unclaimedTerminalEvents.get(terminalId);
    this.unclaimedTerminalEvents.delete(terminalId);
    for (const event of held ?? []) {
      queue.push(event as TerminalOutputEvent);
    }
    return { terminalId, events: this.iterateTerminal(terminalId, queue) };
  }

  private async *iterateTerminal(
    terminalId: string,
    queue: AsyncQueue<TerminalOutputEvent>,
  ): AsyncGenerator<TerminalOutputEvent> {
    try {
      for await (const event of queue) {
        yield event;
        if (event.type === 'exit') {
          return;
        }
      }
    } finally {
      this.eventQueues.delete(terminalId);
    }
  }

  /** Sends keystrokes or pasted bytes to the terminal (at most 64 KiB per call). */
  async terminalInput(terminalId: string, data: Uint8Array): Promise<void> {
    if (data.length > MAX_TERMINAL_INPUT_BYTES) {
      throw new RangeError(`terminal input is limited to ${MAX_TERMINAL_INPUT_BYTES} bytes per call`);
    }
    await this.dispatch('sandbox_terminal_input', {
      terminal_id: terminalId,
      data: bytesToBase64(data),
    });
  }

  async terminalResize(terminalId: string, cols: number, rows: number): Promise<void> {
    await this.dispatch('sandbox_terminal_resize', {
      terminal_id: terminalId,
      cols,
      rows,
    });
  }

  /** Ends the terminal. Safe to repeat. */
  async terminalClose(terminalId: string): Promise<void> {
    await this.dispatch('sandbox_terminal_close', { terminal_id: terminalId });
  }

  async pollCommand(leaseId: LeaseId, executionId: string): Promise<PollCommandResponse> {
    return this.dispatch<PollCommandResponse>('sandbox_poll_command', {
      lease_id: leaseId,
      execution_id: executionId,
    });
  }

  async writeFiles(
    leaseId: LeaseId,
    files: SandboxFileWrite[],
    opts?: { asUser?: string },
  ): Promise<WriteFilesResponse> {
    return this.dispatch<WriteFilesResponse>('sandbox_write_files', {
      lease_id: leaseId,
      files,
      as_user: opts?.asUser,
    });
  }

  async readFile(leaseId: LeaseId, path: string, opts?: { asUser?: string }): Promise<ReadFileResponse> {
    return this.dispatch<ReadFileResponse>('sandbox_read_file', {
      lease_id: leaseId,
      path,
      as_user: opts?.asUser,
    });
  }

  /**
   * Runs one structured file operation. The reply is validated, and a typed refusal
   * (unsupported provider, missing path, failed operation) becomes its matching error.
   * An atomic edit whose occurrence count differs comes back as an `edit_rejected` reply.
   */
  async fileOp(leaseId: LeaseId, op: FileOp, opts?: { asUser?: string }): Promise<FileOpResponse> {
    const raw = await this.dispatch<unknown>('sandbox_file_op', {
      lease_id: leaseId,
      op,
      as_user: opts?.asUser,
    });
    return throwOnRefusal(parseFileOpResponse(raw));
  }

  private unexpectedFileReply(verb: string, reply: FileOpResponse): never {
    throw new FileOpFailedError('io', `unexpected ${reply.outcome} reply to ${verb}`);
  }

  async listDir(
    leaseId: LeaseId,
    path: string,
    opts?: { recursive?: boolean; maxEntries?: number; asUser?: string },
  ): Promise<{ entries: WireFileStat[]; truncated: boolean }> {
    const reply = await this.fileOp(
      leaseId,
      {
        op: 'list',
        path,
        recursive: opts?.recursive,
        max_entries: opts?.maxEntries,
      },
      opts,
    );
    if (reply.outcome !== 'listing') return this.unexpectedFileReply('list', reply);
    return { entries: reply.entries, truncated: reply.truncated };
  }

  async statPath(leaseId: LeaseId, path: string, opts?: { asUser?: string }): Promise<WireFileStat> {
    const reply = await this.fileOp(leaseId, { op: 'stat', path }, opts);
    if (reply.outcome !== 'stat') return this.unexpectedFileReply('stat', reply);
    return reply.stat;
  }

  /** Reads a byte range and reports the file's total size, so large files page through. */
  async readRange(
    leaseId: LeaseId,
    path: string,
    opts?: {
      offset?: number;
      length?: number;
      maxBytes?: number;
      asUser?: string;
    },
  ): Promise<{
    data: Uint8Array;
    offset: number;
    totalSize: number;
    eof: boolean;
    truncated: boolean;
  }> {
    const reply = await this.fileOp(
      leaseId,
      {
        op: 'read',
        path,
        offset: opts?.offset,
        length: opts?.length,
        max_bytes: opts?.maxBytes,
      },
      opts,
    );
    if (reply.outcome !== 'content') return this.unexpectedFileReply('read', reply);
    return {
      data: base64ToBytes(reply.data),
      offset: reply.offset,
      totalSize: reply.total_size,
      eof: reply.eof,
      truncated: reply.truncated,
    };
  }

  /** Atomic exact-text replace. Throws {@link EditRejectedError} without writing on a count mismatch. */
  async editFile(
    leaseId: LeaseId,
    path: string,
    find: string,
    replace: string,
    opts?: { expectedOccurrences?: number; asUser?: string },
  ): Promise<{ occurrences: number; size: number }> {
    const reply = await this.fileOp(
      leaseId,
      {
        op: 'edit',
        path,
        find,
        replace,
        expected_occurrences: opts?.expectedOccurrences,
      },
      opts,
    );
    if (reply.outcome === 'edit_rejected') throw new EditRejectedError(reply.occurrences, reply.expected);
    if (reply.outcome !== 'edited') return this.unexpectedFileReply('edit', reply);
    return { occurrences: reply.occurrences, size: reply.size };
  }

  async searchFiles(
    leaseId: LeaseId,
    path: string,
    opts?: {
      glob?: string;
      pattern?: string;
      regex?: boolean;
      caseInsensitive?: boolean;
      maxResults?: number;
      maxFileBytes?: number;
      asUser?: string;
    },
  ): Promise<{
    matches: WireSearchMatch[];
    truncated: boolean;
    filesScanned: number;
  }> {
    const reply = await this.fileOp(
      leaseId,
      {
        op: 'search',
        path,
        glob: opts?.glob,
        pattern: opts?.pattern,
        regex: opts?.regex,
        case_insensitive: opts?.caseInsensitive,
        max_results: opts?.maxResults,
        max_file_bytes: opts?.maxFileBytes,
      },
      opts,
    );
    if (reply.outcome !== 'search_results') return this.unexpectedFileReply('search', reply);
    return {
      matches: reply.matches,
      truncated: reply.truncated,
      filesScanned: reply.files_scanned,
    };
  }

  async removePath(leaseId: LeaseId, path: string, opts?: { recursive?: boolean; asUser?: string }): Promise<boolean> {
    const reply = await this.fileOp(leaseId, { op: 'remove', path, recursive: opts?.recursive }, opts);
    if (reply.outcome !== 'removed') return this.unexpectedFileReply('remove', reply);
    return reply.existed;
  }

  async makeDir(leaseId: LeaseId, path: string, opts?: { parents?: boolean; asUser?: string }): Promise<void> {
    const reply = await this.fileOp(leaseId, { op: 'mkdir', path, parents: opts?.parents }, opts);
    if (reply.outcome !== 'created') this.unexpectedFileReply('mkdir', reply);
  }

  async renamePath(
    leaseId: LeaseId,
    from: string,
    to: string,
    opts?: { overwrite?: boolean; asUser?: string },
  ): Promise<void> {
    const reply = await this.fileOp(leaseId, { op: 'rename', from, to, overwrite: opts?.overwrite }, opts);
    if (reply.outcome !== 'renamed') this.unexpectedFileReply('rename', reply);
  }

  /** Reads a whole file of any size by paging ranged reads. `signal` stops it between pages. */
  async downloadFile(
    leaseId: LeaseId,
    path: string,
    opts?: { asUser?: string; signal?: AbortSignal },
  ): Promise<Uint8Array> {
    const parts: Uint8Array[] = [];
    let offset = 0;
    for (;;) {
      opts?.signal?.throwIfAborted();
      const range = await this.readRange(leaseId, path, {
        offset,
        asUser: opts?.asUser,
      });
      parts.push(range.data);
      offset += range.data.length;
      if (range.eof || range.data.length === 0) break;
    }
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let at = 0;
    for (const part of parts) {
      out.set(part, at);
      at += part.length;
    }
    return out;
  }

  /**
   * Writes a whole file of any size as chunked writes at increasing offsets. Returns its
   * final size. `signal` stops it between chunks, leaving a partial file.
   */
  async uploadFile(
    leaseId: LeaseId,
    path: string,
    data: Uint8Array,
    opts?: { mode?: number; asUser?: string; signal?: AbortSignal },
  ): Promise<number> {
    let offset = 0;
    let size = 0;
    do {
      opts?.signal?.throwIfAborted();
      const end = Math.min(offset + FILE_TRANSFER_CHUNK_BYTES, data.length);
      const reply = await this.fileOp(
        leaseId,
        {
          op: 'write',
          path,
          data: bytesToBase64(data.subarray(offset, end)),
          offset,
          truncate: offset === 0,
          mode: offset === 0 ? opts?.mode : undefined,
          create_parents: true,
        },
        opts,
      );
      if (reply.outcome !== 'written') return this.unexpectedFileReply('write', reply);
      size = reply.size;
      offset = end;
    } while (offset < data.length);
    return size;
  }

  /**
   * SHA-256 of `[offset, offset + length)` of a sandbox file; omit `length` to hash to the
   * end. Needs a provider that advertises `supports_dir_transfer`.
   */
  async fileChecksum(
    leaseId: LeaseId,
    path: string,
    opts?: { offset?: number; length?: number; asUser?: string },
  ): Promise<{ sha256: string; bytes: number; totalSize: number }> {
    const reply = await this.fileOp(
      leaseId,
      { op: 'checksum', path, offset: opts?.offset, length: opts?.length },
      opts,
    );
    if (reply.outcome !== 'checksum') return this.unexpectedFileReply('checksum', reply);
    return {
      sha256: reply.sha256,
      bytes: reply.bytes,
      totalSize: reply.total_size,
    };
  }

  /** Size of a regular file in the sandbox, 0 when absent. A directory in the way is refused. */
  private async remoteFileSize(leaseId: LeaseId, path: string, opts?: { asUser?: string }): Promise<number> {
    try {
      const stat = await this.statPath(leaseId, path, opts);
      if (stat.kind !== 'file')
        throw new FileOpFailedError('is_a_directory', `${path} exists and is not a regular file`);
      return stat.size;
    } catch (error) {
      if (error instanceof FileNotFoundError) return 0;
      throw error;
    }
  }

  /**
   * Uploads a file in chunks, continuing from the bytes already in the sandbox when they
   * match the start of `data`, then compares whole-file digests. A mismatch throws.
   * `signal` stops it between chunks; a later call resumes from what was written.
   * Needs `supports_dir_transfer`.
   */
  async uploadFileVerified(
    leaseId: LeaseId,
    path: string,
    data: Uint8Array,
    opts?: { mode?: number; asUser?: string; signal?: AbortSignal },
  ): Promise<TransferReport> {
    opts?.signal?.throwIfAborted();
    let start = await this.remoteFileSize(leaseId, path, opts);
    let resumed = false;
    if (start > 0) {
      const reusable =
        start <= data.length &&
        (await this.fileChecksum(leaseId, path, { ...opts, offset: 0, length: start })).sha256 ===
          (await sha256Hex(data.subarray(0, start)));
      if (reusable) resumed = start < data.length;
      else start = 0;
    }
    let moved = 0;
    if (start < data.length || data.length === 0) {
      let offset = start;
      do {
        opts?.signal?.throwIfAborted();
        const end = Math.min(offset + FILE_TRANSFER_CHUNK_BYTES, data.length);
        const reply = await this.fileOp(
          leaseId,
          {
            op: 'write',
            path,
            data: bytesToBase64(data.subarray(offset, end)),
            offset,
            truncate: offset === 0,
            mode: offset === 0 ? opts?.mode : undefined,
            create_parents: true,
          },
          opts,
        );
        if (reply.outcome !== 'written') return this.unexpectedFileReply('write', reply);
        moved += end - offset;
        offset = end;
      } while (offset < data.length);
    }
    const remote = await this.fileChecksum(leaseId, path, opts);
    if (remote.bytes !== data.length || remote.sha256 !== (await sha256Hex(data))) throw integrityError(path);
    const report = emptyReport();
    absorb(report, {
      bytesTransferred: moved,
      resumed,
      skipped: start === data.length && data.length > 0,
    });
    return report;
  }

  /**
   * Downloads a file in pages and compares its digest with the sandbox copy. Pass the bytes
   * of an interrupted earlier download as `partial` to continue after them; a partial copy
   * that does not match the start of the sandbox file is discarded. `signal` stops it
   * between pages. Needs `supports_dir_transfer`.
   */
  async downloadFileVerified(
    leaseId: LeaseId,
    path: string,
    opts?: { partial?: Uint8Array; asUser?: string; signal?: AbortSignal },
  ): Promise<{ data: Uint8Array; report: TransferReport }> {
    opts?.signal?.throwIfAborted();
    const remote = await this.fileChecksum(leaseId, path, opts);
    let prefix = opts?.partial ?? new Uint8Array(0);
    if (prefix.length > 0) {
      const reusable =
        prefix.length <= remote.totalSize &&
        (await this.fileChecksum(leaseId, path, { ...opts, offset: 0, length: prefix.length })).sha256 ===
          (await sha256Hex(prefix));
      if (!reusable) prefix = new Uint8Array(0);
    }
    const resumed = prefix.length > 0 && prefix.length < remote.totalSize;
    const skipped = prefix.length > 0 && prefix.length === remote.totalSize;
    const parts: Uint8Array[] = [prefix];
    let offset = prefix.length;
    let moved = 0;
    while (offset < remote.totalSize) {
      opts?.signal?.throwIfAborted();
      const range = await this.readRange(leaseId, path, {
        offset,
        asUser: opts?.asUser,
      });
      if (range.data.length === 0) break;
      parts.push(range.data);
      offset += range.data.length;
      moved += range.data.length;
      if (range.eof) break;
    }
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let at = 0;
    for (const part of parts) {
      out.set(part, at);
      at += part.length;
    }
    if (out.length !== remote.totalSize || (await sha256Hex(out)) !== remote.sha256) throw integrityError(path);
    const report = emptyReport();
    absorb(report, { bytesTransferred: moved, resumed, skipped });
    return { data: out, report };
  }

  /**
   * Uploads a file from a stream of byte pieces without holding the whole file: pieces are
   * re-cut into transfer chunks and hashed as they go, then the digest is compared with the
   * sandbox copy. A mismatch throws. Needs `supports_dir_transfer`.
   *
   * By default it starts from byte 0. With `resume`, `source` must replay the same bytes
   * from the start: the bytes already in the sandbox are read past without being sent,
   * once their digest matches the sandbox copy, and the upload continues from there. A
   * sandbox copy that is not a prefix of `source` throws rather than being overwritten,
   * since a one-shot stream cannot be rewound; retry without `resume` to start over.
   */
  async uploadFileStream(
    leaseId: LeaseId,
    path: string,
    source: Iterable<Uint8Array> | AsyncIterable<Uint8Array>,
    opts?: { mode?: number; asUser?: string; signal?: AbortSignal; resume?: boolean },
  ): Promise<TransferReport> {
    opts?.signal?.throwIfAborted();
    const start = opts?.resume === true ? await this.remoteFileSize(leaseId, path, opts) : 0;
    const prefixDigest =
      start > 0 ? (await this.fileChecksum(leaseId, path, { ...opts, offset: 0, length: start })).sha256 : undefined;
    const hash = new Sha256();
    const pending = new Uint8Array(FILE_TRANSFER_CHUNK_BYTES);
    let pendingLen = 0;
    let offset = start;
    let seen = 0;
    const flush = async (): Promise<void> => {
      opts?.signal?.throwIfAborted();
      const reply = await this.fileOp(
        leaseId,
        {
          op: 'write',
          path,
          data: bytesToBase64(pending.subarray(0, pendingLen)),
          offset,
          truncate: offset === 0,
          mode: offset === 0 ? opts?.mode : undefined,
          create_parents: true,
        },
        opts,
      );
      if (reply.outcome !== 'written') this.unexpectedFileReply('write', reply);
      offset += pendingLen;
      pendingLen = 0;
    };
    const checkPrefix = (): void => {
      if (hash.clone().digestHex() !== prefixDigest) throw resumeMismatchError(path);
    };
    for await (const piece of source) {
      let at = 0;
      if (seen < start) {
        at = Math.min(start - seen, piece.length);
        hash.update(piece.subarray(0, at));
        seen += at;
        if (seen === start) checkPrefix();
      }
      hash.update(piece.subarray(at));
      seen += piece.length - at;
      while (at < piece.length) {
        const take = Math.min(FILE_TRANSFER_CHUNK_BYTES - pendingLen, piece.length - at);
        pending.set(piece.subarray(at, at + take), pendingLen);
        pendingLen += take;
        at += take;
        if (pendingLen === FILE_TRANSFER_CHUNK_BYTES) await flush();
      }
    }
    if (seen < start) throw resumeMismatchError(path);
    const sent = offset - start + pendingLen;
    if (pendingLen > 0 || offset === 0) await flush();
    const remote = await this.fileChecksum(leaseId, path, opts);
    if (remote.bytes !== offset || remote.sha256 !== hash.digestHex()) throw integrityError(path);
    const report = emptyReport();
    absorb(report, {
      bytesTransferred: sent,
      resumed: start > 0 && start < offset,
      skipped: start > 0 && start === offset,
    });
    return report;
  }

  /**
   * Downloads a file page by page, yielding each page as it arrives and hashing it on the
   * way, so the whole file is never buffered. The read reply carries no digest of its own,
   * so by default the only integrity check is the whole-file digest compared after the last
   * page; a mismatch throws from the iterator, so a consumer must treat what it wrote as
   * unverified until iteration ends cleanly.
   *
   * `verifyPages` compares each page with a ranged checksum of the sandbox copy before
   * yielding it, so a corrupt page throws instead of reaching the consumer. It costs one
   * extra round trip per page. The whole-file check still runs at the end, since the file
   * can change between pages.
   *
   * The generator's return value is the transfer report. Needs `supports_dir_transfer`.
   */
  async *downloadFileStream(
    leaseId: LeaseId,
    path: string,
    opts?: { asUser?: string; signal?: AbortSignal; verifyPages?: boolean },
  ): AsyncGenerator<Uint8Array, TransferReport, void> {
    opts?.signal?.throwIfAborted();
    const remote = await this.fileChecksum(leaseId, path, opts);
    const hash = new Sha256();
    let offset = 0;
    while (offset < remote.totalSize) {
      opts?.signal?.throwIfAborted();
      const range = await this.readRange(leaseId, path, { offset, asUser: opts?.asUser });
      if (range.data.length === 0) break;
      if (opts?.verifyPages === true) {
        const page = await this.fileChecksum(leaseId, path, {
          offset,
          length: range.data.length,
          asUser: opts.asUser,
        });
        if (page.bytes !== range.data.length || page.sha256 !== (await sha256Hex(range.data)))
          throw integrityError(path);
      }
      hash.update(range.data);
      offset += range.data.length;
      yield range.data;
      if (range.eof) break;
    }
    if (offset !== remote.totalSize || hash.digestHex() !== remote.sha256) throw integrityError(path);
    const report = emptyReport();
    absorb(report, { bytesTransferred: offset, resumed: false, skipped: false });
    return report;
  }

  /**
   * Copies `files` into the sandbox directory `dir`, creating it. Each file is resumable
   * and verified like {@link uploadFileVerified}. Paths must stay inside `dir`.
   * `emptyDirs` names directories to create even though they hold no file. `signal`
   * stops it between chunks.
   */
  async uploadDir(
    leaseId: LeaseId,
    dir: string,
    files: Iterable<DirFile> | AsyncIterable<DirFile>,
    opts?: { emptyDirs?: string[]; asUser?: string; signal?: AbortSignal },
  ): Promise<TransferReport> {
    opts?.signal?.throwIfAborted();
    const report = emptyReport();
    await this.makeDir(leaseId, dir, { parents: true, asUser: opts?.asUser });
    report.directories += 1;
    for (const emptyDir of opts?.emptyDirs ?? []) {
      await this.makeDir(leaseId, joinRemote(dir, checkRelative(emptyDir)), {
        parents: true,
        asUser: opts?.asUser,
      });
      report.directories += 1;
    }
    for await (const file of files) {
      const outcome = await this.uploadFileVerified(leaseId, joinRemote(dir, checkRelative(file.path)), file.data, {
        mode: file.mode,
        asUser: opts?.asUser,
        signal: opts?.signal,
      });
      report.files += outcome.files;
      report.bytesTransferred += outcome.bytesTransferred;
      report.resumedFiles += outcome.resumedFiles;
      report.skippedFiles += outcome.skippedFiles;
    }
    return report;
  }

  /**
   * Reads every regular file under the sandbox directory `dir`, each verified by digest.
   * Symlinks and special files are skipped. A directory with more entries than one
   * listing carries is refused rather than silently truncated. `signal` stops it between
   * pages.
   */
  async downloadDir(
    leaseId: LeaseId,
    dir: string,
    opts?: { asUser?: string; signal?: AbortSignal },
  ): Promise<DownloadedDir> {
    opts?.signal?.throwIfAborted();
    const listing = await this.listDir(leaseId, dir, {
      recursive: true,
      maxEntries: FILE_OP_MAX_ENTRIES,
      asUser: opts?.asUser,
    });
    if (listing.truncated)
      throw new FileOpFailedError(
        'too_large',
        `${dir} holds more than ${FILE_OP_MAX_ENTRIES} entries; transfer its subdirectories separately`,
      );
    const entries = [...listing.entries].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    const report = emptyReport();
    const files: DirFile[] = [];
    const directories: string[] = [];
    for (const entry of entries) {
      const relative = checkRelative(entry.path);
      if (entry.kind === 'directory') {
        directories.push(relative);
        report.directories += 1;
      } else if (entry.kind === 'file') {
        const { data, report: one } = await this.downloadFileVerified(leaseId, joinRemote(dir, relative), opts);
        files.push({ path: relative, data, mode: entry.mode });
        report.files += one.files;
        report.bytesTransferred += one.bytesTransferred;
      }
    }
    return { files, directories, report };
  }

  /** Packs a file or directory tree into one tar stream. */
  async downloadArchive(
    leaseId: LeaseId,
    path: string,
    opts?: { maxBytes?: number; asUser?: string },
  ): Promise<Uint8Array> {
    const reply = await this.fileOp(leaseId, { op: 'archive_get', path, max_bytes: opts?.maxBytes }, opts);
    if (reply.outcome !== 'archive') return this.unexpectedFileReply('archive_get', reply);
    return base64ToBytes(reply.data);
  }

  /** Unpacks a tar stream into the directory `path`. Returns the number of entries extracted. */
  async uploadArchive(
    leaseId: LeaseId,
    path: string,
    archive: Uint8Array,
    opts?: { asUser?: string },
  ): Promise<number> {
    const reply = await this.fileOp(leaseId, { op: 'archive_put', path, data: bytesToBase64(archive) }, opts);
    if (reply.outcome !== 'extracted') return this.unexpectedFileReply('archive_put', reply);
    return reply.entries;
  }

  /**
   * Records the tree under `path` as a baseline for `fileChanges` and returns its id.
   * `hash` also fingerprints content, so a rewrite with identical bytes is not a change.
   * Needs a provider with `supports_change_tracking`.
   */
  async checkpointFiles(
    leaseId: LeaseId,
    path: string,
    opts?: { hash?: boolean },
  ): Promise<{ checkpointId: string; entries: number }> {
    const reply = await this.fileOp(leaseId, { op: 'checkpoint', path, hash: opts?.hash });
    if (reply.outcome !== 'checkpointed') return this.unexpectedFileReply('checkpoint', reply);
    return { checkpointId: reply.checkpoint_id, entries: reply.entries };
  }

  /**
   * Paths added, modified and deleted since a checkpoint, or since the lease start when
   * `since` is omitted. The result is the net difference, so a file created and removed
   * in between is not listed. `path` narrows it to one subtree.
   */
  async fileChanges(leaseId: LeaseId, opts?: { since?: string; path?: string }): Promise<FileChangeSet> {
    const reply = await this.fileOp(leaseId, {
      op: 'changes',
      since: opts?.since,
      path: opts?.path,
    });
    if (reply.outcome !== 'changes') return this.unexpectedFileReply('changes', reply);
    return {
      since: reply.since,
      added: reply.added,
      modified: reply.modified,
      deleted: reply.deleted,
      truncated: reply.truncated,
    };
  }

  /**
   * Watches `path` and yields create, modify and delete events as they happen, until the
   * `signal` aborts or the consumer stops iterating. A yielded `overflowed` item means events
   * were dropped before they were read. Providers that cannot watch throw
   * `FileOpUnsupportedError` on the first iteration. The watch is stopped on exit; one
   * abandoned without a clean exit expires at the worker.
   */
  async *watchFiles(
    leaseId: LeaseId,
    path: string,
    opts?: { recursive?: boolean; signal?: AbortSignal },
  ): AsyncGenerator<FileWatchItem, void, void> {
    const started = await this.fileOp(leaseId, {
      op: 'watch_start',
      path,
      recursive: opts?.recursive ?? false,
    });
    if (started.outcome !== 'watch_started') return this.unexpectedFileReply('watch_start', started);
    const watchId = started.watch_id;
    let cursor = 0;
    try {
      while (opts?.signal?.aborted !== true) {
        const reply = await this.fileOp(leaseId, {
          op: 'watch_poll',
          watch_id: watchId,
          after_seq: cursor,
          wait_ms: FILE_WATCH_POLL_WAIT_MS,
        });
        if (reply.outcome !== 'watch_events') return this.unexpectedFileReply('watch_poll', reply);
        cursor = Math.max(cursor, reply.next_seq);
        if (reply.overflowed) yield { type: 'overflowed' };
        for (const event of reply.events) {
          yield { type: 'event', event };
        }
      }
    } finally {
      await this.fileOp(leaseId, { op: 'watch_stop', watch_id: watchId }).catch(() => undefined);
    }
  }

  async killCommand(leaseId: LeaseId, executionId: string, signal?: UnixSignal): Promise<KillCommandResponse> {
    return this.dispatch<KillCommandResponse>('sandbox_kill_command', {
      lease_id: leaseId,
      execution_id: executionId,
      signal,
    });
  }

  /**
   * Stops a detached command gracefully: SIGTERM, then SIGKILL once
   * `graceSeconds` (default 10, capped at 300) has passed. Resolves
   * `{ outcome: 'unsupported' }` when the provider cannot.
   */
  async stopCommand(leaseId: LeaseId, executionId: string, graceSeconds?: number): Promise<StopCommandResponse> {
    return this.dispatch<StopCommandResponse>('sandbox_stop_command', {
      lease_id: leaseId,
      execution_id: executionId,
      grace_seconds: graceSeconds,
    });
  }

  /**
   * Stops a detached command and runs it again with the same process spec.
   * The new run has a new execution id. Resolves `{ outcome: 'unsupported' }`
   * when the provider cannot.
   */
  async restartCommand(leaseId: LeaseId, executionId: string, graceSeconds?: number): Promise<RestartCommandResponse> {
    return this.dispatch<RestartCommandResponse>('sandbox_restart_command', {
      lease_id: leaseId,
      execution_id: executionId,
      grace_seconds: graceSeconds,
    });
  }

  /**
   * Reads one page of a command's durable output after the cursor `afterSeq`
   * (0 starts at the oldest retained chunk). Resume with the page's
   * `next_seq`. Resolves `{ outcome: 'unsupported' }` when the broker keeps no
   * durable logs.
   */
  async readLogs(
    leaseId: LeaseId,
    executionId: string,
    opts?: { afterSeq?: number; maxBytes?: number; waitMs?: number },
  ): Promise<ReadLogsResponse> {
    return this.dispatch<ReadLogsResponse>('sandbox_read_logs', {
      lease_id: leaseId,
      execution_id: executionId,
      after_seq: opts?.afterSeq,
      max_bytes: opts?.maxBytes,
      wait_ms: opts?.waitMs,
    });
  }

  /**
   * Follows a command's output from the cursor `afterSeq` (0 starts at the
   * oldest retained chunk): the retained chunks first, then live ones, each
   * with its `log_seq` cursor. The last event yielded is `end` once the
   * command has exited and all of its output was delivered. If the
   * connection drops first the iteration throws a `TransportError`; call
   * again with the last `log_seq` seen to resume without gaps or repeats.
   * One follow per command per connection at a time.
   */
  async *followLogs(
    leaseId: LeaseId,
    executionId: string,
    opts?: { afterSeq?: number },
  ): AsyncGenerator<LogFollowEvent, void, void> {
    const streamId = `logs:${String(leaseId)}:${executionId}`;
    if (this.eventQueues.has(streamId)) {
      throw new SandboxWsError('invalid_request', `already following the output of ${executionId}`);
    }
    const queue = this.createQueue<LogFollowEvent>(streamId);
    try {
      await this.dispatch<{ following: true; stream_id: string }>('sandbox_follow_logs', {
        lease_id: leaseId,
        execution_id: executionId,
        after_seq: opts?.afterSeq,
      });
      for await (const event of queue) {
        yield event;
        if (event.type !== 'chunk') {
          return;
        }
      }
      throw new TransportError('websocket connection closed while following command output', {
        tool: 'sandbox_follow_logs',
        leaseId,
      });
    } finally {
      this.eventQueues.delete(streamId);
    }
  }

  async stopLease(leaseId: LeaseId): Promise<StopLeaseResponse> {
    return this.dispatch<StopLeaseResponse>('sandbox_stop_lease', {
      lease_id: leaseId,
    });
  }

  /**
   * Pauses a lease in place, keeping its id and state. The lease is not reaped
   * while paused, up to `maxPausedSeconds` (server default one hour).
   * Idempotent. Resolves `{ outcome: 'unsupported' }` when the provider cannot
   * pause.
   */
  async pauseLease(leaseId: LeaseId, maxPausedSeconds?: number): Promise<PauseLeaseResponse> {
    return this.dispatch<PauseLeaseResponse>('sandbox_pause_lease', {
      lease_id: leaseId,
      max_paused_seconds: maxPausedSeconds,
    });
  }

  /** Usage counters for a lease; a released lease returns its final figures. Read-only. */
  async leaseUsage(leaseId: LeaseId): Promise<LeaseUsage> {
    return this.dispatch<LeaseUsage>('sandbox_usage', { lease_id: leaseId });
  }

  /** Usage summed over the tenant's live leases. Read-only. */
  async tenantUsage(): Promise<TenantUsage> {
    return this.dispatch<TenantUsage>('sandbox_tenant_usage', {});
  }

  /**
   * The tenant's limits and what it has consumed: live leases against the cap,
   * monthly cpu and wall budgets, per-request caps and any per-tag cap with
   * its live counts. Read-only; use it to check capacity before creating a lease.
   */
  async limits(): Promise<AccountLimits> {
    return this.dispatch<AccountLimits>('sandbox_account_limits', {});
  }

  /** Resumes a paused lease with the run time it had left. Idempotent. */
  async resumeLease(leaseId: LeaseId): Promise<ResumeLeaseResponse> {
    return this.dispatch<ResumeLeaseResponse>('sandbox_resume_lease', {
      lease_id: leaseId,
    });
  }

  /**
   * Suspends a lease: its host resources are released while its id and state
   * stay, which costs less than a pause. `level` is `auto` (default), `memory`
   * (processes continue after wake) or `disk` (only the disk survives); the
   * last two must match what the provider offers. The lease is reaped after
   * `maxSuspendedSeconds` (server default 24 hours). Idempotent. Rejects with
   * a `lease_denied` error of kind `suspend_unsupported` when the provider
   * cannot suspend to that level.
   */
  async suspendLease(
    leaseId: LeaseId,
    opts: { level?: SuspendRequestLevel; maxSuspendedSeconds?: number } = {},
  ): Promise<SuspendLeaseResponse> {
    return this.dispatch<SuspendLeaseResponse>('sandbox_suspend_lease', {
      lease_id: leaseId,
      ...(opts.level === undefined ? {} : { level: opts.level }),
      ...(opts.maxSuspendedSeconds === undefined ? {} : { max_suspended_seconds: opts.maxSuspendedSeconds }),
    });
  }

  /**
   * Wakes a suspended lease with the run time it had left. Idempotent: a
   * running lease is returned unchanged. Rejects with a `lease_denied` error
   * of kind `lease_waking`, which carries `retryAfterMs`, while another wake
   * is in flight.
   */
  async wakeLease(leaseId: LeaseId): Promise<WakeLeaseResponse> {
    return this.dispatch<WakeLeaseResponse>('sandbox_wake_lease', {
      lease_id: leaseId,
    });
  }

  async deleteLease(leaseId: LeaseId): Promise<DeleteLeaseResponse> {
    return this.dispatch<DeleteLeaseResponse>('sandbox_delete_lease', {
      lease_id: leaseId,
    });
  }

  async extendLeaseTimeout(leaseId: LeaseId, additionalSeconds: number): Promise<ExtendLeaseTimeoutResponse> {
    return this.dispatch<ExtendLeaseTimeoutResponse>('sandbox_extend_lease_timeout', {
      lease_id: leaseId,
      additional_seconds: additionalSeconds,
    });
  }

  async fork(sourceLeaseId: LeaseId, ttlSeconds: number, idempotencyKey: string): Promise<LeaseGranted> {
    return this.dispatch<LeaseGranted>('sandbox_fork', {
      source_lease_id: sourceLeaseId,
      ttl_seconds: ttlSeconds,
      idempotency_key: idempotencyKey,
    });
  }

  async updateNetworkPolicy(leaseId: LeaseId, networkPolicy: NetworkPolicy): Promise<NetworkPolicyUpdateResponse> {
    return this.dispatch<NetworkPolicyUpdateResponse>('sandbox_update_network_policy', {
      lease_id: leaseId,
      network_policy: networkPolicy,
    });
  }

  async updateTags(leaseId: LeaseId, tags: SandboxTags): Promise<UpdateTagsResponse> {
    return this.dispatch<UpdateTagsResponse>('sandbox_update_tags', {
      lease_id: leaseId,
      tags,
    });
  }

  /**
   * Creates a drive. `storage` is an S3 bucket name, or a `ProviderVolumeRef`
   * naming an existing provider-native volume.
   */
  async createDrive(
    driveName: string,
    storage: string | ProviderVolumeRef,
    opts?: { sizeGib?: number },
  ): Promise<CreateDriveResponse> {
    return this.dispatch<CreateDriveResponse>('sandbox_create_drive', {
      drive_name: driveName,
      ...(typeof storage === 'string' ? { bucket: storage } : { provider_volume: storage }),
      size_gib: opts?.sizeGib,
    });
  }

  async listDrives(): Promise<DriveRecord[]> {
    const response = await this.dispatch<ListDrivesResponse>('sandbox_list_drives', {});
    return response.drives;
  }

  async deleteDrive(driveName: string): Promise<DeleteDriveResponse> {
    return this.dispatch<DeleteDriveResponse>('sandbox_delete_drive', {
      drive_name: driveName,
    });
  }

  async getSandboxDomain(leaseId: LeaseId, port: number, waitMs?: number): Promise<GetSandboxDomainResponse> {
    return this.dispatch<GetSandboxDomainResponse>('sandbox_get_domain', {
      lease_id: leaseId,
      port,
      ...(waitMs === undefined ? {} : { wait_ms: waitMs }),
    });
  }

  /** The vendor's own sandbox id and URL. Needs the tenant to be allowed native passthrough. */
  async getNativeHandle(leaseId: LeaseId): Promise<NativeHandleResponse> {
    return this.dispatch<NativeHandleResponse>('sandbox_native_handle', {
      lease_id: leaseId,
    });
  }

  /**
   * Mints direct SSH access to a sandbox, valid for about `ttlSeconds` when
   * the provider honours a lifetime. Needs the tenant to be allowed native
   * passthrough.
   */
  async getSshAccess(leaseId: LeaseId, ttlSeconds?: number): Promise<SshAccessResponse> {
    return this.dispatch<SshAccessResponse>('sandbox_ssh_access', {
      lease_id: leaseId,
      ...(ttlSeconds === undefined ? {} : { ttl_seconds: ttlSeconds }),
    });
  }

  /**
   * Permanently deletes a sandbox by its tenant-scoped name, including one
   * with no active lease (for example after `stopLease`).
   */
  async deleteNamedSandbox(name: string): Promise<DeleteNamedSandboxResponse> {
    return this.dispatch<DeleteNamedSandboxResponse>('sandbox_delete_named_sandbox', {
      name,
    });
  }

  /**
   * Reads a persistent sandbox entity, including a stopped one. Rejects with
   * `entity_not_enabled` when the broker runs with `SANDBOX_ENTITY_MODE=off`.
   */
  async getEntity(entity: EntityRef): Promise<GetEntityResponse> {
    return this.dispatch<GetEntityResponse>('sandbox_get_entity', entityPayload(entity));
  }

  /** Lists the tenant's persistent sandbox entities, ordered by id, one page at a time. */
  async listEntities(options: ListEntitiesOptions = {}): Promise<ListEntitiesResponse> {
    return this.dispatch<ListEntitiesResponse>('sandbox_list_entities', {
      ...(options.limit === undefined ? {} : { limit: options.limit }),
      ...(options.cursor === undefined ? {} : { cursor: options.cursor }),
    });
  }

  /**
   * Permanently deletes a persistent sandbox entity with its running lease,
   * snapshots and name. Needs `SANDBOX_ENTITY_MODE=on`.
   */
  async deleteEntity(entity: EntityRef): Promise<DeleteEntityResponse> {
    return this.dispatch<DeleteEntityResponse>('sandbox_delete_entity', entityPayload(entity));
  }

  async getLeaseByName(name: string): Promise<GetLeaseByNameResponse> {
    return this.dispatch<GetLeaseByNameResponse>('sandbox_get_lease_by_name', {
      name,
    });
  }

  /**
   * Snapshots a running lease without stopping it. The receipt's `reference`
   * starts a new lease as `{ kind: 'snapshot', reference }`. Resolves
   * `{ outcome: 'unsupported' }` when the provider cannot checkpoint a running
   * lease. With `acceptNative`, a provider that snapshots natively resolves
   * `{ outcome: 'created_native' }`; without it that provider is `unsupported`.
   */
  async snapshotLease(leaseId: LeaseId, opts?: { acceptNative?: boolean }): Promise<SnapshotLeaseResponse> {
    return this.dispatch<SnapshotLeaseResponse>('sandbox_snapshot_lease', {
      lease_id: leaseId,
      ...(opts?.acceptNative === true ? { accept_native: true } : {}),
    });
  }

  /** Deletes a snapshot by digest. A repeat resolves `{ outcome: 'not_found' }`. */
  async snapshotDelete(digest: Digest): Promise<SnapshotDeleteResponse> {
    return this.dispatch<SnapshotDeleteResponse>('sandbox_snapshot_delete', {
      digest,
    });
  }

  /** Retires an owned provider-native snapshot. Denial is a typed gateway error. */
  async nativeSnapshotDelete(reference: NativeSnapshotRef): Promise<NativeSnapshotDeleteResponse> {
    const parsed = parseNativeSnapshotReference(reference);
    const response = await this.dispatch<unknown>('sandbox_native_snapshot_delete', { reference: parsed });
    return parseNativeSnapshotDeleteResponse(response);
  }

  async operationCapabilities(): Promise<OperationCapabilities> {
    return parseOperationResponse(
      operationCapabilitiesSchema,
      await this.dispatch<unknown>('sandbox_operation_capabilities', {}),
    );
  }

  async cancelOperation(operationId: string, opts: CancelOperationOptions = {}): Promise<CancelOperationResponse> {
    const id = parseOperationId(operationId);
    const parsed = parseCancelOperationOptions(opts);
    return parseOperationResponse(
      cancelOperationResponseSchema,
      await this.dispatch<unknown>('sandbox_cancel_operation', {
        operation_id: id,
        ...(parsed.compensateCreated === undefined ? {} : { compensate_created: parsed.compensateCreated }),
      }),
      true,
    );
  }

  async getOperation(operationId: string): Promise<GetOperationResponse> {
    const id = parseOperationId(operationId);
    return parseOperationResponse(
      getOperationResponseSchema,
      await this.dispatch<unknown>('sandbox_get_operation', { operation_id: id }),
    );
  }

  async snapshotGet(digest: Digest): Promise<SnapshotGetResponse> {
    return this.dispatch<SnapshotGetResponse>('sandbox_snapshot_get', {
      digest,
    });
  }

  async snapshotTree(digest: Digest): Promise<SnapshotTreeResponse> {
    return this.dispatch<SnapshotTreeResponse>('sandbox_snapshot_tree', {
      digest,
    });
  }

  async setSnapshotTag(name: string, digest: Digest): Promise<SetSnapshotTagResponse> {
    return this.dispatch<SetSnapshotTagResponse>('sandbox_set_snapshot_tag', {
      name,
      digest,
    });
  }

  async deleteSnapshotTag(name: string): Promise<DeleteSnapshotTagResponse> {
    return this.dispatch<DeleteSnapshotTagResponse>('sandbox_delete_snapshot_tag', { name });
  }

  async snapshotTagHistory(name: string): Promise<SnapshotTagHistoryResponse> {
    return this.dispatch<SnapshotTagHistoryResponse>('sandbox_snapshot_tag_history', { name });
  }

  async createUser(leaseId: LeaseId, username: string): Promise<SandboxUser> {
    assertValidSandboxName(username, 'username');
    await this.dispatch<CreateSandboxUserResponse>('sandbox_create_user', {
      lease_id: leaseId,
      username,
    });
    return new SandboxUser(this, leaseId, username);
  }

  asUser(leaseId: LeaseId, username: string): SandboxUser {
    assertValidSandboxName(username, 'username');
    return new SandboxUser(this, leaseId, username);
  }

  async createGroup(leaseId: LeaseId, groupname: string): Promise<{ sharedDir: string }> {
    assertValidSandboxName(groupname, 'groupname');
    const response = await this.dispatch<CreateSandboxGroupResponse>('sandbox_create_group', {
      lease_id: leaseId,
      groupname,
    });
    return { sharedDir: response.shared_dir };
  }

  async addUserToGroup(leaseId: LeaseId, username: string, groupname: string): Promise<void> {
    await this.modifyGroupMembership(leaseId, username, groupname, 'add');
  }

  async removeUserFromGroup(leaseId: LeaseId, username: string, groupname: string): Promise<void> {
    await this.modifyGroupMembership(leaseId, username, groupname, 'remove');
  }

  async modifyGroupMembership(
    leaseId: LeaseId,
    username: string,
    groupname: string,
    action: GroupMembershipAction,
  ): Promise<void> {
    assertValidSandboxName(username, 'username');
    assertValidSandboxName(groupname, 'groupname');
    await this.dispatch('sandbox_modify_group_membership', {
      lease_id: leaseId,
      username,
      groupname,
      action,
    });
  }

  evidenceStream(leaseId: LeaseId): AsyncIterable<EvidenceEvent> {
    return this.iterateEvidence(this.subscribeEvidence(leaseId));
  }

  /**
   * Registers a listener on the lease's evidence stream. The gateway is
   * subscribed once per lease no matter how many listeners attach; `ready`
   * resolves once that subscription is acknowledged. Events that arrive
   * before iteration starts are buffered. `close` detaches this listener.
   */
  subscribeEvidence(leaseId: LeaseId): EvidenceSubscription {
    const streamId = String(leaseId);
    const queue = new AsyncQueue<{ subject_suffix: string; payload: string }>();
    const listener: EventQueue = {
      push: (event) => queue.push(event as never),
      end: () => queue.end(),
    };
    let listeners = this.evidenceListeners.get(streamId);
    if (!listeners) {
      listeners = new Set();
      this.evidenceListeners.set(streamId, listeners);
    }
    listeners.add(listener);
    let ready = this.evidenceSubscriptions.get(streamId);
    if (!ready) {
      const subscribed: Promise<void> = this.dispatch<{ subscribed: true }>('sandbox_subscribe_evidence', {
        lease_id: leaseId,
      }).then(
        () => undefined,
        (err) => {
          if (this.evidenceSubscriptions.get(streamId) === subscribed) {
            this.evidenceSubscriptions.delete(streamId);
          }
          throw err;
        },
      );
      ready = subscribed;
      this.evidenceSubscriptions.set(streamId, ready);
    }
    ready.catch(() => undefined);
    const events = mapIterable(queue, (raw) => ({
      subjectSuffix: raw.subject_suffix,
      payload: decodeEvidencePayload(raw.payload),
    }));
    const close = () => {
      const current = this.evidenceListeners.get(streamId);
      current?.delete(listener);
      if (current?.size === 0) {
        this.evidenceListeners.delete(streamId);
      }
      listener.end();
    };
    return { ready, events, close };
  }

  private async *iterateEvidence(subscription: EvidenceSubscription): AsyncGenerator<EvidenceEvent> {
    try {
      await subscription.ready;
      yield* subscription.events;
    } finally {
      subscription.close();
    }
  }

  async listCapabilities(): Promise<CapabilitiesResponse> {
    return this.dispatch<CapabilitiesResponse>('sandbox_list_capabilities', {});
  }

  /** Lists the session tenant's leases; with `tags`, only those whose tags contain every entry. */
  async listLeases(opts?: { tags?: SandboxTags }): Promise<{ leases: LeaseListRow[] }> {
    const payload: Record<string, unknown> = {};
    if (opts?.tags !== undefined && Object.keys(opts.tags).length > 0) {
      payload.tags = opts.tags;
    }
    return this.dispatch<{ leases: LeaseListRow[] }>('sandbox_list_leases', payload);
  }

  /**
   * Releases the session tenant's leases matching every filter, or only
   * reports them with `dry_run`. Safe to repeat. The gateway always uses the
   * session tenant.
   */
  async reapLeases(opts: ReapLeasesOptions): Promise<ReapLeasesResponse> {
    return this.dispatch<ReapLeasesResponse>('sandbox_reap_leases', {
      ...opts,
    });
  }

  async listSnapshots(): Promise<{ snapshots: SnapshotListRow[] }> {
    return this.dispatch<{ snapshots: SnapshotListRow[] }>('sandbox_list_snapshots', {});
  }

  async listTags(): Promise<{ tags: SnapshotTagRow[] }> {
    return this.dispatch<{ tags: SnapshotTagRow[] }>('sandbox_list_tags', {});
  }

  subscribeDashboard(): AsyncIterable<DashboardEvent> {
    const queues = DASHBOARD_STREAM_IDS.map((streamId) => this.createQueue<unknown>(streamId));
    const subscribed = this.dispatch<{ subscribed: true; streams: string[] }>('sandbox_subscribe_dashboard', {});
    return this.iterateMergedDashboardQueues(queues, subscribed);
  }

  close(): void {
    this.socket.close();
    this.handleClose();
  }

  private createQueue<T>(streamId: string): AsyncQueue<T> {
    const queue = new AsyncQueue<T>();
    this.eventQueues.set(streamId, {
      push: (event) => queue.push(event as T),
      end: () => queue.end(),
    });
    return queue;
  }

  private async *iterateMergedDashboardQueues(
    queues: Array<AsyncQueue<unknown>>,
    subscribed: Promise<unknown>,
  ): AsyncGenerator<DashboardEvent> {
    await subscribed;
    const [leases, snapshots, workers] = queues;
    const iterators = [
      { stream: 'leases' as const, iterator: leases[Symbol.asyncIterator]() },
      {
        stream: 'snapshots' as const,
        iterator: snapshots[Symbol.asyncIterator](),
      },
      { stream: 'workers' as const, iterator: workers[Symbol.asyncIterator]() },
    ];
    const pulls = new Map(
      iterators.map(({ stream, iterator }) => [stream, iterator.next().then((result) => ({ stream, result }))]),
    );
    while (pulls.size > 0) {
      const { stream, result } = await Promise.race(pulls.values());
      if (result.done) {
        pulls.delete(stream);
        continue;
      }
      const entry = iterators.find((i) => i.stream === stream)!;
      pulls.set(
        stream,
        entry.iterator.next().then((next) => ({ stream, result: next })),
      );
      yield shapeDashboardEvent(stream, result.value);
    }
  }
}

async function* mapIterable<Raw, Out>(source: AsyncIterable<Raw>, map: (raw: Raw) => Out): AsyncGenerator<Out> {
  for await (const raw of source) {
    yield map(raw);
  }
}

function shapeDashboardEvent(stream: 'leases' | 'snapshots' | 'workers', event: unknown): DashboardEvent {
  if (stream === 'leases') {
    return { stream, ...(event as LeaseDashboardEvent) };
  }
  if (stream === 'snapshots') {
    return { stream, ...(event as SnapshotPublishedEvent) };
  }
  return { stream, ...(event as WorkerHeartbeatEvent) };
}

function decodeEvidencePayload(payload: string): string | Uint8Array {
  const prefix = 'base64:';
  if (!payload.startsWith(prefix)) {
    return payload;
  }
  const encoded = payload.slice(prefix.length);
  const binary = atob(encoded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

class AsyncQueue<T> implements AsyncIterable<T> {
  private readonly buffer: T[] = [];
  private readonly waiters: Array<(result: IteratorResult<T>) => void> = [];
  private ended = false;

  push(value: T): void {
    if (this.ended) {
      return;
    }
    const waiter = this.waiters.shift();
    if (waiter) {
      waiter({ value, done: false });
    } else {
      this.buffer.push(value);
    }
  }

  end(): void {
    this.ended = true;
    while (this.waiters.length > 0) {
      const waiter = this.waiters.shift()!;
      waiter({ value: undefined, done: true });
    }
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return {
      next: (): Promise<IteratorResult<T>> => {
        if (this.buffer.length > 0) {
          return Promise.resolve({
            value: this.buffer.shift() as T,
            done: false,
          });
        }
        if (this.ended) {
          return Promise.resolve({ value: undefined, done: true });
        }
        return new Promise((resolve) => {
          this.waiters.push(resolve);
        });
      },
    };
  }
}

import { z } from 'zod';

import type { SandboxFeature } from './capabilities.ts';
import type { SandboxClientApi } from './client-api.ts';
import type { FileChangeSet, FileWatchItem } from './client.ts';
import { parseCancelCommandLaunchRequest, requireCommandLaunchCancellation } from './command-launch.ts';
import type { CancelCommandLaunchResponse } from './command-launch.ts';
import { SandboxWsError } from './errors.ts';
import {
  bytesToBase64,
  EditRejectedError,
  FILE_TRANSFER_CHUNK_BYTES,
  FileNotFoundError,
  FileOpFailedError,
} from './fileops.ts';
import type { Digest, LeaseId } from './ids.ts';
import { parseNativeSnapshotDeleteResponse, parseNativeSnapshotReference } from './native-snapshots.ts';
import type {
  CancelOperationOptions,
  CancelOperationResponse,
  GetOperationResponse,
  OperationCapabilities,
} from './operations.ts';
import { parseCancelOperationOptions, parseOperationId } from './operations.ts';
import { processNameSchema, processReadinessSchema } from './process-state.ts';
import { redactText, redactValuesSchema } from './redact.ts';
import { fileEditInputSchema, fileListInputSchema, fileSearchInputSchema, parseFileInput } from './structured-files.ts';
import { absorb, checkRelative, emptyReport, joinRemote, sha256Hex } from './transfer.ts';
import type { DirFile, DownloadedDir, TransferReport } from './transfer.ts';
import { capabilityRequirementsSchema } from './wire.ts';
import type {
  ReapLeasesOptions,
  ReapLeasesResponse,
  ExtendLeaseTimeoutResponse,
  PauseLeaseResponse,
  ResumeLeaseResponse,
  SuspendLeaseResponse,
  SuspendRequestLevel,
  WakeLeaseResponse,
  SnapshotDeleteResponse,
  NativeSnapshotDeleteResponse,
  NativeSnapshotRef,
  SnapshotLeaseResponse,
  LeaseUsage,
  TenantUsage,
  AccountLimits,
  GetLeaseByNameResponse,
  DeleteEntityResponse,
  DeleteNamedSandboxResponse,
  EntityRef,
  GetEntityResponse,
  ListEntitiesOptions,
  ListEntitiesResponse,
  KillCommandResponse,
  ListProcessesResponse,
  LogFollowEvent,
  ReadLogsResponse,
  RestartCommandResponse,
  StopCommandResponse,
  WaitReadyResponse,
  WaitSandboxReadyResponse,
  LeaseGranted,
  LeaseListRow,
  LeaseRequest,
  PollCommandResponse,
  ProviderKind,
  ReadFileResponse,
  RunCommandResponse,
  SandboxFileWrite,
  UnixSignal,
  WriteFilesResponse,
  WireFileStat,
  WireSearchMatch,
  ProcessSpec,
  CommandDeadline,
} from './wire.ts';

/**
 * In-process stand-in for `SandboxWsClient`: a virtual filesystem and a table
 * of scripted command handlers, no network and no gateway. Pass it wherever a
 * `SandboxClientApi` is accepted, for example `Sandbox.create(client, opts)`.
 *
 * Behaviour mirrors the wire as far as the wire is modelled here: an unknown
 * command exits 127, reading a missing file throws a "No such file" error, and
 * detached commands are polled to completion. Verbs this fake does not model
 * throw a `SandboxWsError` with code `unsupported` instead of pretending to
 * succeed; see {@link MEMORY_CAPABILITIES}.
 */

/** What the fake models, declared so callers can branch on it like on a real profile. */
export const MEMORY_CAPABILITIES = {
  provider: 'mock' as ProviderKind,
  supported: [
    'lease',
    'run_command',
    'run_command_detached',
    'poll_command',
    'kill_command',
    'stop_command',
    'restart_command',
    'list_processes',
    'write_files',
    'read_file',
    'stop_lease',
    'pause_lease',
    'resume_lease',
    'suspend_lease',
    'wake_lease',
    'get_usage',
    'file_checkpoint',
    'file_changes',
    'file_watch',
    'file_list',
    'file_edit',
    'file_search',
    'delete_lease',
    'extend_lease_timeout',
    'update_tags',
    'get_lease_by_name',
    'list_leases',
    'snapshot_lease',
    'snapshot_delete',
  ],
  unsupported: [
    'cancel_operation',
    'command_cancellation',
    'command_launch_cancellation',
    'cancel_command_launch',
    'native_snapshot_delete',
    'fork',
    'write_stdin',
    'terminal',
    'read_logs',
    'update_network_policy',
    'get_sandbox_domain',
    'native_handle',
    'create_user',
    'create_group',
    'modify_group_membership',
    'drives',
    'evidence_streaming',
  ],
} as const;

export interface CommandResult {
  exitCode: number;
  stdout?: string | Uint8Array;
  stderr?: string | Uint8Array;
}

export interface CommandContext {
  /** Arguments after the command name. */
  args: string[];
  cwd: string;
  env: Record<string, string>;
  fs: MemoryFileSystem;
  /** Aborted when the command is killed or times out. */
  signal: AbortSignal;
}

export type CommandHandler = (ctx: CommandContext) => CommandResult | Promise<CommandResult>;

function toBytes(data: string | Uint8Array | undefined): Uint8Array {
  if (data === undefined) {
    return new Uint8Array();
  }
  return typeof data === 'string' ? new TextEncoder().encode(data) : data;
}

function normalizePath(cwd: string, path: string): string {
  const parts: string[] = [];
  for (const part of (path.startsWith('/') ? path : `${cwd}/${path}`).split('/')) {
    if (part === '' || part === '.') {
      continue;
    }
    if (part === '..') {
      parts.pop();
    } else {
      parts.push(part);
    }
  }
  return `/${parts.join('/')}`;
}

function noSuchFile(path: string): SandboxWsError {
  return new SandboxWsError('not_found', `${path}: No such file or directory`);
}

function memoryGlobMatch(path: string, glob: string): boolean {
  if (/[[\]{}\\]/.test(glob)) {
    throw new SandboxWsError('unsupported', 'the memory fake models only literal, *, ** and ? glob syntax');
  }
  let previous = new Uint8Array(path.length + 1);
  previous[0] = 1;
  for (let index = 0; index < glob.length; index += 1) {
    const current = new Uint8Array(path.length + 1);
    const token = glob[index];
    const globstar = token === '*' && glob[index + 1] === '*';
    const directories = globstar && glob[index + 2] === '/';
    let prefix = false;
    if (token === '*') current[0] = previous[0];
    for (let at = 1; at <= path.length; at += 1) {
      prefix ||= previous[at - 1] === 1;
      const character = path[at - 1];
      current[at] = Number(
        directories
          ? previous[at] === 1 || (character === '/' && prefix)
          : token === '*'
            ? previous[at] === 1 || (current[at - 1] === 1 && (globstar || character !== '/'))
            : previous[at - 1] === 1 && (token === '?' ? character !== '/' : character === token),
      );
    }
    if (globstar) index += directories ? 2 : 1;
    previous = current;
  }
  return previous[path.length] === 1;
}

/** Path-keyed file map plus a directory set. Relative paths resolve against `cwd`. */
export class MemoryFileSystem {
  private readonly files = new Map<string, { data: Uint8Array; mode: number }>();
  private readonly dirs = new Set<string>(['/']);
  /** Bumped by every mutation, so a watcher can tell whether anything happened since it looked. */
  private version = 0;
  private readonly waiters = new Set<() => void>();
  private readonly checkpoints = new Map<string, Map<string, string>>();
  private checkpointCounter = 0;

  constructor(readonly cwd = '/') {}

  private touched(): void {
    this.version += 1;
    const waiters = [...this.waiters];
    for (const wake of waiters) {
      wake();
    }
  }

  /** Content fingerprints of the files under `root` (directories are implicit). */
  private snapshot(root: string, recursive = true): Map<string, string> {
    const out = new Map<string, string>();
    const prefix = root === '/' ? '/' : `${root}/`;
    for (const [path, entry] of this.files) {
      const inside =
        path === root || (path.startsWith(prefix) && (recursive || !path.slice(prefix.length).includes('/')));
      if (inside) {
        out.set(path, bytesToBase64(entry.data));
      }
    }
    return out;
  }

  /**
   * Records a baseline of the files under `path` and returns its id and entry count. A
   * path that does not exist yet gets an empty baseline, so a directory created later
   * reports its files as added.
   */
  checkpoint(path: string): { checkpointId: string; entries: number } {
    const root = this.resolve(path);
    this.checkpointCounter += 1;
    const checkpointId = `cp-${this.checkpointCounter}`;
    const snap = this.snapshot(root);
    this.checkpoints.set(checkpointId, snap);
    return { checkpointId, entries: snap.size };
  }

  /**
   * Net difference since a checkpoint, or since the lease start (an empty tree) when
   * `since` is omitted. Exact: files only, and rewriting identical bytes is not a change.
   */
  changes(since?: string, path?: string): FileChangeSet {
    const base = since === undefined ? new Map<string, string>() : this.checkpoints.get(since);
    if (base === undefined) {
      throw new SandboxWsError('invalid_request', `unknown checkpoint ${since}`);
    }
    const scope = path === undefined ? '/' : this.resolve(path);
    const inScope = (p: string) => scope === '/' || p === scope || p.startsWith(`${scope}/`);
    const now = this.snapshot('/');
    const added: string[] = [];
    const modified: string[] = [];
    const deleted: string[] = [];
    for (const [p, fingerprint] of now) {
      if (!inScope(p)) continue;
      const before = base.get(p);
      if (before === undefined) added.push(p);
      else if (before !== fingerprint) modified.push(p);
    }
    for (const p of base.keys()) {
      if (inScope(p) && !now.has(p)) deleted.push(p);
    }
    return {
      since: since ?? 'start',
      added: added.sort(),
      modified: modified.sort(),
      deleted: deleted.sort(),
      truncated: false,
    };
  }

  /** Events under `path` as they happen; ends when `signal` aborts. */
  async *watch(
    path: string,
    opts?: { recursive?: boolean; signal?: AbortSignal },
  ): AsyncGenerator<FileWatchItem, void, void> {
    const root = this.resolve(path);
    if (!this.exists(root)) {
      throw noSuchFile(path);
    }
    const recursive = opts?.recursive ?? false;
    const signal = opts?.signal;
    let previous = this.snapshot(root, recursive);
    let seenVersion = this.version;
    let seq = 0;
    while (signal?.aborted !== true) {
      if (this.version === seenVersion) {
        await new Promise<void>((resolve) => {
          const wake = () => {
            this.waiters.delete(wake);
            signal?.removeEventListener('abort', wake);
            resolve();
          };
          this.waiters.add(wake);
          signal?.addEventListener('abort', wake, { once: true });
        });
        continue;
      }
      seenVersion = this.version;
      const current = this.snapshot(root, recursive);
      const events: Array<{ kind: 'created' | 'modified' | 'deleted'; path: string }> = [];
      for (const [p, fingerprint] of current) {
        const before = previous.get(p);
        if (before === undefined) events.push({ kind: 'created', path: p });
        else if (before !== fingerprint) events.push({ kind: 'modified', path: p });
      }
      for (const p of previous.keys()) {
        if (!current.has(p)) events.push({ kind: 'deleted', path: p });
      }
      previous = current;
      events.sort((a, b) => a.path.localeCompare(b.path));
      for (const event of events) {
        seq += 1;
        yield { type: 'event', event: { seq, kind: event.kind, path: event.path, is_dir: false } };
      }
    }
  }

  resolve(path: string, cwd = this.cwd): string {
    return normalizePath(cwd, path);
  }

  write(path: string, data: string | Uint8Array, opts?: { mode?: number; cwd?: string }): void {
    const full = this.resolve(path, opts?.cwd);
    this.mkdirp(full.slice(0, full.lastIndexOf('/')) || '/');
    this.files.set(full, { data: Uint8Array.from(toBytes(data)), mode: opts?.mode ?? 0o644 });
    this.touched();
  }

  read(path: string, cwd?: string): Uint8Array {
    const entry = this.files.get(this.resolve(path, cwd));
    if (entry === undefined) {
      throw noSuchFile(path);
    }
    return Uint8Array.from(entry.data);
  }

  mkdirp(path: string, cwd?: string): void {
    const parts = this.resolve(path, cwd).split('/').filter(Boolean);
    let current = '';
    for (const part of parts) {
      current += `/${part}`;
      this.dirs.add(current);
    }
  }

  exists(path: string, cwd?: string): boolean {
    const full = this.resolve(path, cwd);
    return this.files.has(full) || this.dirs.has(full);
  }

  isDir(path: string, cwd?: string): boolean {
    return this.dirs.has(this.resolve(path, cwd));
  }

  /** A copy of every file and directory, for snapshots. */
  capture(): { files: Map<string, { data: Uint8Array; mode: number }>; dirs: Set<string> } {
    const files = new Map<string, { data: Uint8Array; mode: number }>();
    for (const [path, entry] of this.files) {
      files.set(path, { data: Uint8Array.from(entry.data), mode: entry.mode });
    }
    return { files, dirs: new Set(this.dirs) };
  }

  /** Replaces the contents with a copy of a {@link capture}. */
  restore(state: { files: Map<string, { data: Uint8Array; mode: number }>; dirs: Set<string> }): void {
    this.files.clear();
    this.dirs.clear();
    for (const [path, entry] of state.files) {
      this.files.set(path, { data: Uint8Array.from(entry.data), mode: entry.mode });
    }
    for (const dir of state.dirs) {
      this.dirs.add(dir);
    }
  }

  /** Files and directories under `path`, relative to it, sorted. */
  tree(path: string): { files: Array<{ path: string; data: Uint8Array; mode: number }>; dirs: string[] } {
    const root = this.resolve(path);
    if (!this.dirs.has(root)) {
      throw noSuchFile(path);
    }
    const prefix = root === '/' ? '/' : `${root}/`;
    const files = [...this.files]
      .filter(([key]) => key.startsWith(prefix))
      .map(([key, entry]) => ({ path: key.slice(prefix.length), data: Uint8Array.from(entry.data), mode: entry.mode }))
      .sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    const dirs = [...this.dirs]
      .filter((key) => key.startsWith(prefix))
      .map((key) => key.slice(prefix.length))
      .sort();
    return { files, dirs };
  }

  /** Immediate children names of a directory, sorted. */
  list(path: string, cwd?: string): string[] {
    const full = this.resolve(path, cwd);
    if (!this.dirs.has(full)) {
      throw noSuchFile(path);
    }
    const prefix = full === '/' ? '/' : `${full}/`;
    const names = new Set<string>();
    for (const key of [...this.files.keys(), ...this.dirs]) {
      if (key !== full && key.startsWith(prefix)) {
        names.add(key.slice(prefix.length).split('/')[0]);
      }
    }
    return [...names].sort();
  }

  listEntries(path: string, recursive: boolean, maxEntries: number): { entries: WireFileStat[]; truncated: boolean } {
    const root = this.resolve(path);
    if (!this.exists(root)) throw noSuchFile(path);
    if (!this.isDir(root)) throw new FileOpFailedError('not_a_directory', 'listing requires a directory');
    const prefix = root === '/' ? '/' : `${root}/`;
    const paths = [...new Set([...this.files.keys(), ...this.dirs])]
      .filter((candidate) => {
        if (candidate === root || !candidate.startsWith(prefix)) return false;
        return recursive || !candidate.slice(prefix.length).includes('/');
      })
      .sort();
    const entries = paths.slice(0, maxEntries).map((candidate): WireFileStat => {
      const file = this.files.get(candidate);
      return {
        path: candidate.slice(prefix.length),
        name: candidate.slice(candidate.lastIndexOf('/') + 1),
        kind: file === undefined ? 'directory' : 'file',
        size: file?.data.byteLength ?? 0,
        ...(file === undefined ? {} : { mode: file.mode }),
      };
    });
    return { entries, truncated: paths.length > maxEntries };
  }

  remove(path: string, opts?: { recursive?: boolean; cwd?: string }): boolean {
    const full = this.resolve(path, opts?.cwd);
    let removed = this.files.delete(full);
    if (opts?.recursive) {
      const prefix = `${full}/`;
      const fileKeys = [...this.files.keys()];
      for (const key of fileKeys) {
        if (key.startsWith(prefix)) {
          this.files.delete(key);
          removed = true;
        }
      }
      const directories = [...this.dirs];
      for (const key of directories) {
        if (key !== '/' && (key === full || key.startsWith(prefix))) {
          this.dirs.delete(key);
          removed = true;
        }
      }
    }
    if (removed) {
      this.touched();
    }
    return removed;
  }
}

function failure(command: string, message: string, exitCode = 1): CommandResult {
  return { exitCode, stderr: `${command}: ${message}\n` };
}

/** The handlers every memory sandbox starts with, enough for `Sandbox.mkDir` and `Sandbox.fs`. */
function builtinHandlers(): Record<string, CommandHandler> {
  return {
    echo: ({ args }) => ({ exitCode: 0, stdout: `${args.join(' ')}\n` }),
    true: () => ({ exitCode: 0 }),
    false: () => ({ exitCode: 1 }),
    pwd: ({ cwd }) => ({ exitCode: 0, stdout: `${cwd}\n` }),
    cat: ({ args, fs, cwd }) => {
      const chunks: Uint8Array[] = [];
      for (const path of args) {
        try {
          chunks.push(fs.read(path, cwd));
        } catch {
          return failure('cat', `${path}: No such file or directory`);
        }
      }
      const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
      let offset = 0;
      for (const c of chunks) {
        out.set(c, offset);
        offset += c.length;
      }
      return { exitCode: 0, stdout: out };
    },
    mkdir: ({ args, fs, cwd }) => {
      for (const path of args.filter((a) => !a.startsWith('-'))) {
        fs.mkdirp(path, cwd);
      }
      return { exitCode: 0 };
    },
    rm: ({ args, fs, cwd }) => {
      const flags = args.filter((a) => a.startsWith('-')).join('');
      for (const path of args.filter((a) => !a.startsWith('-'))) {
        const removed = fs.remove(path, { recursive: flags.includes('r'), cwd });
        if (!removed && !flags.includes('f')) {
          return failure('rm', `cannot remove '${path}': No such file or directory`);
        }
      }
      return { exitCode: 0 };
    },
    test: ({ args, fs, cwd }) => {
      const [flag, path] = args;
      if (path === undefined) {
        return { exitCode: 1 };
      }
      const ok =
        flag === '-e'
          ? fs.exists(path, cwd)
          : flag === '-d'
            ? fs.isDir(path, cwd)
            : flag === '-f'
              ? fs.exists(path, cwd) && !fs.isDir(path, cwd)
              : false;
      return { exitCode: ok ? 0 : 1 };
    },
    ls: ({ args, fs, cwd }) => {
      const path = args.filter((a) => !a.startsWith('-')).at(-1) ?? '.';
      try {
        const names = fs.list(path, cwd);
        return { exitCode: 0, stdout: names.map((n) => `${n}\n`).join('') };
      } catch {
        return failure('ls', `cannot access '${path}': No such file or directory`, 2);
      }
    },
  };
}

const SIGNAL_NUMBERS: Record<UnixSignal, number> = {
  hup: 1,
  int: 2,
  quit: 3,
  kill: 9,
  usr1: 10,
  usr2: 12,
  term: 15,
};

const ABSENT = Symbol('absent');

const leaseRequestSchema = z.object({
  tenant: z.string().min(1),
  ttl_seconds: z.number().int().positive(),
  idempotency_key: z.string().min(1),
  name: z.string().min(1).optional(),
  tags: z.record(z.string(), z.string()).optional(),
  spec: z
    .object({
      env: z.record(z.string(), z.string()).optional(),
      working_dir: z.string().optional(),
      image: z.object({ kind: z.string() }).loose().optional(),
    })
    .loose(),
  native: z.record(z.string(), z.unknown()).optional(),
  region: z.string().min(1).optional(),
  fallback_regions: z.array(z.string().min(1)).optional(),
  required_capabilities: capabilityRequirementsSchema.optional(),
});

const MEMORY_REQUIREMENTS: ReadonlySet<string> = new Set([
  'needs_snapshot',
  'needs_checkpoint_running',
  'needs_restore',
  'needs_named_tags',
  'needs_pause_resume',
  'needs_graceful_stop',
  'needs_output_redaction',
  'needs_change_tracking',
  'needs_file_watch',
  'needs_dir_transfer',
  'needs_named_processes',
  'needs_command_restart',
  'needs_file_search',
]);

const MEMORY_FEATURES = [
  'pause_resume',
  'named_processes',
  'restart_command',
  'snapshots',
  'change_tracking',
  'file_watch',
  'dir_transfer',
  'graceful_stop',
  'file_search',
] as const satisfies readonly SandboxFeature[];

function requireMemoryCapabilities(requirements: LeaseRequest['required_capabilities']): void {
  if (requirements === undefined) return;
  const unsupported =
    Object.entries(requirements).some(([key, value]) => value === true && !MEMORY_REQUIREMENTS.has(key)) ||
    requirements.needs_suspend === 'disk' ||
    requirements.min_isolation_tier !== undefined ||
    requirements.max_cold_restore_p50_ms !== undefined ||
    requirements.vcpu_millis !== undefined ||
    requirements.disk_mib !== undefined ||
    (requirements.exposed_port_count ?? 0) > 0 ||
    (requirements.min_egress_enforcement !== undefined && requirements.min_egress_enforcement !== 'none') ||
    requirements.env_digest !== undefined ||
    requirements.region !== undefined;
  if (unsupported) {
    throw new SandboxWsError('lease_denied', 'the memory fake cannot satisfy the requested capabilities', {
      variant: 'no_worker_satisfies_capabilities',
    });
  }
}

const runCommandSchema = z.object({
  command: z
    .array(z.string())
    .min(1)
    .refine((c) => (c[0] ?? 'x').length > 0, 'command name must not be empty'),
  cwd: z.string().min(1).optional(),
  env: z.record(z.string(), z.string()).optional(),
  timeoutSeconds: z.number().int().positive().optional(),
  detached: z.boolean().optional(),
  redactValues: redactValuesSchema.optional(),
  process: z
    .strictObject({ name: processNameSchema.optional(), readiness: processReadinessSchema.optional() })
    .optional(),
  deadline: z.enum(['timeout', 'lease']).optional(),
});

const writeFilesSchema = z.array(
  z.object({
    path: z.string().min(1),
    content: z.array(z.number().int().min(0).max(255)),
    mode: z.number().int().min(0).max(0o7777).optional(),
  }),
);

const pathSchema = z.string().min(1);
const durationSchema = z.number().int().positive();

function parse<T extends z.ZodType>(schema: T, value: unknown, what: string): z.output<T> {
  const result = schema.safeParse(value);
  if (!result.success) {
    const detail = result.error.issues.map((i) => `${i.path.join('.') || '<root>'}: ${i.message}`).join('; ');
    throw new SandboxWsError('invalid_request', `${what}: ${detail}`);
  }
  return result.data;
}

function unsupported(verb: string): SandboxWsError {
  return new SandboxWsError('unsupported', `${verb} is not supported by the in-memory sandbox`);
}

function toWireTime(ms: number): { secs_since_epoch: number; nanos_since_epoch: number } {
  return { secs_since_epoch: Math.floor(ms / 1000), nanos_since_epoch: (ms % 1000) * 1e6 };
}

interface Execution {
  result?: { exitCode: number; stdout: Uint8Array; stderr: Uint8Array };
  abort: AbortController;
  killed: boolean;
  input: z.output<typeof runCommandSchema>;
}

interface MemoryLease {
  id: LeaseId;
  tenant: string;
  name?: string;
  tags: Record<string, string>;
  env: Record<string, string>;
  fs: MemoryFileSystem;
  startedAtMs: number;
  deadlineMs: number;
  released: boolean;
  endedAtMs?: number;
  lastActivityMs: number;
  commandsRun: number;
  commandOutputBytes: number;
  fileBytesIn: number;
  fileBytesOut: number;
  pausedMsTotal: number;
  pausedSinceMs?: number;
  /** Deadline left when paused, restored on resume. */
  pausedRemainingMs?: number;
  suspendedMsTotal: number;
  suspendedSinceMs?: number;
  /** Deadline left when suspended, restored on wake. */
  suspendedRemainingMs?: number;
  executions: Map<string, Execution>;
  /** Launches by `launchKey`: the command they ran and their reply, replayed on retry. */
  launches: Map<string, { identity: string; reply: Promise<RunCommandResponse> }>;
}

interface MemorySnapshot {
  tenant: string;
  bytes: number;
  state: ReturnType<MemoryFileSystem['capture']>;
}

/** How long a released lease's final figures stay readable, matching the broker's usage store. */
const USAGE_RETENTION_MS = 7 * 24 * 60 * 60 * 1000;

export interface MemorySandboxClientOptions {
  /** Extra or overriding command handlers, keyed by command name. */
  commands?: Record<string, CommandHandler>;
  /** Defaults to a counter, so ids are stable across runs of the same test. */
  nextId?: () => string;
  now?: () => number;
}

export class MemorySandboxClient implements SandboxClientApi {
  private readonly handlers = new Map<string, CommandHandler>();
  private readonly leases = new Map<LeaseId, MemoryLease>();
  private readonly byIdempotencyKey = new Map<string, LeaseId>();
  private readonly snapshots = new Map<string, MemorySnapshot>();
  private counter = 0;
  private readonly nextId: () => string;
  private readonly now: () => number;

  constructor(opts: MemorySandboxClientOptions = {}) {
    for (const [name, handler] of Object.entries({ ...builtinHandlers(), ...opts.commands })) {
      this.handlers.set(name, handler);
    }
    this.nextId = opts.nextId ?? (() => `mem-${++this.counter}`);
    this.now = opts.now ?? (() => Date.now());
  }

  /** Registers or replaces a command handler. */
  registerCommand(name: string, handler: CommandHandler): void {
    this.handlers.set(name, handler);
  }

  /** The virtual filesystem of a lease, for seeding and asserting in tests. */
  filesystem(leaseId: LeaseId): MemoryFileSystem {
    return this.getLease(leaseId).fs;
  }

  private getLease(leaseId: LeaseId): MemoryLease {
    const lease = this.leases.get(leaseId);
    if (lease === undefined || lease.released) {
      throw new SandboxWsError('not_found', `lease ${leaseId} not found`);
    }
    return lease;
  }

  async checkpointFiles(
    leaseId: LeaseId,
    path: string,
    _opts?: { hash?: boolean },
  ): Promise<{ checkpointId: string; entries: number }> {
    return this.getLease(leaseId).fs.checkpoint(path);
  }

  async fileChanges(leaseId: LeaseId, opts?: { since?: string; path?: string }): Promise<FileChangeSet> {
    return this.getLease(leaseId).fs.changes(opts?.since, opts?.path);
  }

  watchFiles(
    leaseId: LeaseId,
    path: string,
    opts?: { recursive?: boolean; signal?: AbortSignal },
  ): AsyncGenerator<FileWatchItem, void, void> {
    return this.getLease(leaseId).fs.watch(path, opts);
  }

  async fileChecksum(
    leaseId: LeaseId,
    path: string,
    opts?: { offset?: number; length?: number },
  ): Promise<{ sha256: string; bytes: number; totalSize: number }> {
    const fs = this.getLease(leaseId).fs;
    if (!fs.exists(path) || fs.isDir(path)) {
      throw new FileNotFoundError(path);
    }
    const data = fs.read(path);
    const start = Math.min(opts?.offset ?? 0, data.length);
    const end = opts?.length === undefined ? data.length : Math.min(start + opts.length, data.length);
    return { sha256: await sha256Hex(data.subarray(start, end)), bytes: end - start, totalSize: data.length };
  }

  async uploadFileVerified(
    leaseId: LeaseId,
    path: string,
    data: Uint8Array,
    opts?: { mode?: number; signal?: AbortSignal },
  ): Promise<TransferReport> {
    opts?.signal?.throwIfAborted();
    const lease = this.getLease(leaseId);
    const report = emptyReport();
    const same =
      lease.fs.exists(path) &&
      !lease.fs.isDir(path) &&
      (await sha256Hex(lease.fs.read(path))) === (await sha256Hex(data));
    if (!same) {
      lease.fs.write(path, Uint8Array.from(data), { mode: opts?.mode });
      lease.fileBytesIn += data.length;
    }
    absorb(report, { bytesTransferred: same ? 0 : data.length, resumed: false, skipped: same && data.length > 0 });
    return report;
  }

  async downloadFileVerified(
    leaseId: LeaseId,
    path: string,
    opts?: { partial?: Uint8Array; signal?: AbortSignal },
  ): Promise<{ data: Uint8Array; report: TransferReport }> {
    opts?.signal?.throwIfAborted();
    const lease = this.getLease(leaseId);
    if (!lease.fs.exists(path) || lease.fs.isDir(path)) {
      throw new FileNotFoundError(path);
    }
    const data = lease.fs.read(path);
    lease.fileBytesOut += data.length;
    const report = emptyReport();
    absorb(report, { bytesTransferred: data.length, resumed: false, skipped: false });
    return { data, report };
  }

  async uploadFileStream(
    leaseId: LeaseId,
    path: string,
    source: Iterable<Uint8Array> | AsyncIterable<Uint8Array>,
    opts?: { mode?: number; signal?: AbortSignal; resume?: boolean },
  ): Promise<TransferReport> {
    opts?.signal?.throwIfAborted();
    const pieces: Uint8Array[] = [];
    for await (const piece of source) {
      opts?.signal?.throwIfAborted();
      pieces.push(Uint8Array.from(piece));
    }
    const data = new Uint8Array(pieces.reduce((n, p) => n + p.length, 0));
    let at = 0;
    for (const piece of pieces) {
      data.set(piece, at);
      at += piece.length;
    }
    const lease = this.getLease(leaseId);
    lease.fs.write(path, data, { mode: opts?.mode });
    lease.fileBytesIn += data.length;
    const report = emptyReport();
    absorb(report, { bytesTransferred: data.length, resumed: false, skipped: false });
    return report;
  }

  /** Pages come from an in-process copy that cannot be corrupted, so `verifyPages` has nothing to add. */
  async *downloadFileStream(
    leaseId: LeaseId,
    path: string,
    opts?: { signal?: AbortSignal; verifyPages?: boolean },
  ): AsyncGenerator<Uint8Array, TransferReport, void> {
    const { data, report } = await this.downloadFileVerified(leaseId, path, opts);
    for (let at = 0; at < data.length; at += FILE_TRANSFER_CHUNK_BYTES) {
      opts?.signal?.throwIfAborted();
      yield data.subarray(at, Math.min(at + FILE_TRANSFER_CHUNK_BYTES, data.length));
    }
    return report;
  }

  async uploadDir(
    leaseId: LeaseId,
    dir: string,
    files: Iterable<DirFile> | AsyncIterable<DirFile>,
    opts?: { emptyDirs?: string[]; signal?: AbortSignal },
  ): Promise<TransferReport> {
    opts?.signal?.throwIfAborted();
    const lease = this.getLease(leaseId);
    const report = emptyReport();
    lease.fs.mkdirp(dir);
    report.directories += 1;
    for (const emptyDir of opts?.emptyDirs ?? []) {
      lease.fs.mkdirp(joinRemote(dir, checkRelative(emptyDir)));
      report.directories += 1;
    }
    for await (const file of files) {
      const one = await this.uploadFileVerified(leaseId, joinRemote(dir, checkRelative(file.path)), file.data, {
        mode: file.mode,
        signal: opts?.signal,
      });
      report.files += one.files;
      report.bytesTransferred += one.bytesTransferred;
      report.skippedFiles += one.skippedFiles;
    }
    return report;
  }

  async downloadDir(leaseId: LeaseId, dir: string, opts?: { signal?: AbortSignal }): Promise<DownloadedDir> {
    opts?.signal?.throwIfAborted();
    const lease = this.getLease(leaseId);
    const tree = lease.fs.tree(dir);
    const report = emptyReport();
    report.directories = tree.dirs.length;
    report.files = tree.files.length;
    for (const file of tree.files) {
      report.bytesTransferred += file.data.length;
      lease.fileBytesOut += file.data.length;
    }
    return { files: tree.files, directories: tree.dirs, report };
  }

  async lease(request: LeaseRequest): Promise<LeaseGranted> {
    const parsed = parse(leaseRequestSchema, request, 'lease request');
    requireMemoryCapabilities(parsed.required_capabilities);
    // The fake has no vendor and no regions, so it refuses both the way a
    // deny-by-default tenant policy and a region-less fleet would.
    if (parsed.native !== undefined) {
      throw new SandboxWsError(
        'native_passthrough_not_permitted',
        'the in-memory sandbox has no vendor to pass options to',
      );
    }
    if (parsed.region !== undefined) {
      throw new SandboxWsError(
        'no_worker_in_region',
        `the in-memory sandbox serves no region (asked for ${parsed.region})`,
      );
    }
    const existing = this.byIdempotencyKey.get(parsed.idempotency_key);
    const known = existing === undefined ? undefined : this.leases.get(existing);
    if (known !== undefined && !known.released) {
      return this.granted(known, false);
    }
    const seed = this.snapshotSeed(parsed.tenant, parsed.spec.image);
    const id = this.nextId() as LeaseId;
    const startedAtMs = this.now();
    const lease: MemoryLease = {
      id,
      tenant: parsed.tenant,
      name: parsed.name,
      tags: { ...parsed.tags },
      env: { ...parsed.spec.env },
      fs: new MemoryFileSystem(parsed.spec.working_dir ?? '/'),
      startedAtMs,
      deadlineMs: startedAtMs + parsed.ttl_seconds * 1000,
      released: false,
      lastActivityMs: startedAtMs,
      commandsRun: 0,
      commandOutputBytes: 0,
      fileBytesIn: 0,
      fileBytesOut: 0,
      pausedMsTotal: 0,
      suspendedMsTotal: 0,
      executions: new Map(),
      launches: new Map(),
    };
    if (seed !== undefined) {
      lease.fs.restore(seed.state);
    }
    this.leases.set(id, lease);
    this.byIdempotencyKey.set(parsed.idempotency_key, id);
    return this.granted(lease, true);
  }

  /** The snapshot a lease request starts from, or undefined for a plain image. Unknown or foreign digests are denied. */
  private snapshotSeed(tenant: string, image: { kind: string } | undefined): MemorySnapshot | undefined {
    if (image?.kind !== 'snapshot') {
      return undefined;
    }
    const digest = (image as unknown as { reference?: { digest?: string } }).reference?.digest;
    const snapshot = digest === undefined ? undefined : this.snapshots.get(digest);
    if (snapshot === undefined || snapshot.tenant !== tenant) {
      throw new SandboxWsError('lease_denied', `snapshot ${digest ?? '(none)'} not found`, {
        variant: 'invalid_spec',
      });
    }
    return snapshot;
  }

  async snapshotLease(leaseId: LeaseId, _opts?: { acceptNative?: boolean }): Promise<SnapshotLeaseResponse> {
    const lease = this.getLease(leaseId);
    const state = lease.fs.capture();
    let bytes = 0;
    for (const entry of state.files.values()) {
      bytes += entry.data.byteLength;
    }
    const digest = `sha256:${(this.snapshots.size + 1).toString(16).padStart(64, '0')}`;
    this.snapshots.set(digest, { tenant: lease.tenant, bytes, state });
    return {
      outcome: 'created',
      receipt: {
        reference: {
          registry: 'memory',
          repository: `snapshots/${lease.tenant}`,
          digest: digest as Digest,
          artifact_type: 'sandbox_rootfs_v1',
        },
        bytes,
      },
    };
  }

  async snapshotDelete(digest: Digest): Promise<SnapshotDeleteResponse> {
    const snapshot = this.snapshots.get(digest);
    if (snapshot === undefined) {
      return { outcome: 'not_found' };
    }
    this.snapshots.delete(digest);
    return { outcome: 'deleted', bytes: snapshot.bytes };
  }

  async nativeSnapshotDelete(reference: NativeSnapshotRef): Promise<NativeSnapshotDeleteResponse> {
    parseNativeSnapshotReference(reference);
    return parseNativeSnapshotDeleteResponse({ outcome: 'unsupported' });
  }

  async operationCapabilities(): Promise<OperationCapabilities> {
    return { supports_create_cancellation: false };
  }

  async cancelOperation(operationId: string, opts: CancelOperationOptions = {}): Promise<CancelOperationResponse> {
    parseOperationId(operationId);
    parseCancelOperationOptions(opts);
    return { outcome: 'unsupported' };
  }

  async getOperation(operationId: string): Promise<GetOperationResponse> {
    parseOperationId(operationId);
    return { outcome: 'not_found' };
  }

  private granted(lease: MemoryLease, created: boolean): LeaseGranted {
    return {
      lease_id: lease.id,
      provider: MEMORY_CAPABILITIES.provider,
      sandbox_id: `memory-${lease.id}`,
      evidence_subject: `sandbox.evidence.${lease.id}`,
      deadline: toWireTime(lease.deadlineMs),
      created,
      capabilities: [...MEMORY_FEATURES],
    };
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
      redactValues?: string[];
      process?: ProcessSpec;
      deadline?: CommandDeadline;
      /**
       * Runs the command once per key and lease, like the broker: a retry with
       * the same key replays the first launch's reply, and the same key for a
       * different command is refused.
       */
      launchKey?: string;
      requireLaunchCancellation?: boolean;
    },
  ): Promise<RunCommandResponse> {
    if (requireCommandLaunchCancellation(opts?.requireLaunchCancellation, opts?.detached)) {
      throw new SandboxWsError('unsupported', 'the memory provider has no durable command launch custody');
    }
    const launchKey = opts?.launchKey;
    if (launchKey === undefined) {
      return this.launchCommand(leaseId, command, opts);
    }
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(launchKey)) {
      throw new SandboxWsError('invalid_request', 'launchKey: must be 1 to 128 characters of [A-Za-z0-9_-]');
    }
    const lease = this.getLease(leaseId);
    const rest = { ...opts };
    delete rest.launchKey;
    const identity = JSON.stringify([command, rest]);
    const seen = lease.launches.get(launchKey);
    if (seen !== undefined) {
      if (seen.identity !== identity) {
        throw new SandboxWsError('lease_denied', 'launch_key was already used for a different command on this lease');
      }
      return seen.reply;
    }
    const reply = this.launchCommand(leaseId, command, opts);
    lease.launches.set(launchKey, { identity, reply });
    // A refused launch frees its key, as the broker does, so a fixed retry can run.
    reply.catch(() => {
      if (lease.launches.get(launchKey)?.reply === reply) {
        lease.launches.delete(launchKey);
      }
    });
    return reply;
  }

  async cancelCommandLaunch(leaseId: LeaseId, launchKey: string): Promise<CancelCommandLaunchResponse> {
    parseCancelCommandLaunchRequest({ lease_id: leaseId, launch_key: launchKey });
    return { outcome: 'unsupported' };
  }

  private async launchCommand(
    leaseId: LeaseId,
    command: string[],
    opts?: {
      cwd?: string;
      env?: Record<string, string>;
      timeoutSeconds?: number;
      detached?: boolean;
      redactValues?: string[];
      process?: ProcessSpec;
      deadline?: CommandDeadline;
    },
  ): Promise<RunCommandResponse> {
    const input = parse(
      runCommandSchema,
      {
        command,
        cwd: opts?.cwd,
        env: opts?.env,
        timeoutSeconds: opts?.timeoutSeconds,
        detached: opts?.detached,
        redactValues: opts?.redactValues,
        process: opts?.process,
        deadline: opts?.deadline,
      },
      'run command',
    );
    const lease = this.getLease(leaseId);
    if (input.process !== undefined && !input.detached) {
      throw new SandboxWsError('invalid_request', 'named processes require a detached command');
    }
    if (input.process?.readiness !== undefined) {
      throw new SandboxWsError('unsupported', 'the memory fake does not model process readiness probes');
    }
    if (input.process?.name !== undefined) {
      const existing = [...lease.executions].find(
        ([, execution]) => execution.result === undefined && execution.input.process?.name === input.process!.name,
      );
      if (existing !== undefined) return { outcome: 'started', execution_id: existing[0], origin: 'reused' };
    }
    const executionId = `exec-${lease.executions.size + 1}`;
    const execution: Execution = { abort: new AbortController(), killed: false, input };
    lease.executions.set(executionId, execution);
    lease.commandsRun += 1;
    lease.lastActivityMs = this.now();
    const done = this.execute(lease, execution, input);
    if (input.detached) {
      return { outcome: 'started', execution_id: executionId, origin: 'created' };
    }
    await done;
    const result = execution.result!;
    lease.commandOutputBytes += result.stdout.length + result.stderr.length;
    return { outcome: 'completed', exit_code: result.exitCode, stdout: [...result.stdout], stderr: [...result.stderr] };
  }

  private async execute(
    lease: MemoryLease,
    execution: Execution,
    input: z.output<typeof runCommandSchema>,
  ): Promise<void> {
    const [name, ...args] = input.command;
    const handler = this.handlers.get(name);
    let result: Execution['result'];
    if (handler === undefined) {
      result = { exitCode: 127, stdout: new Uint8Array(), stderr: toBytes(`${name}: command not found\n`) };
    } else {
      const leaseRemainingMs = Math.max(0, lease.deadlineMs - this.now());
      const timeoutMs =
        input.deadline === 'lease' ? leaseRemainingMs : Math.min((input.timeoutSeconds ?? 60) * 1000, leaseRemainingMs);
      let timer: ReturnType<typeof setTimeout> | undefined;
      const timedOut = new Promise<typeof ABSENT>((resolve) => {
        timer = setTimeout(() => {
          execution.abort.abort();
          resolve(ABSENT);
        }, timeoutMs);
        timer.unref?.();
      });
      try {
        const outcome = await Promise.race([
          Promise.resolve().then(() =>
            handler({
              args,
              cwd: input.cwd ?? lease.fs.cwd,
              env: { ...lease.env, ...input.env },
              fs: lease.fs,
              signal: execution.abort.signal,
            }),
          ),
          timedOut,
        ]);
        result =
          outcome === ABSENT
            ? { exitCode: 124, stdout: new Uint8Array(), stderr: toBytes(`${name}: timed out\n`) }
            : { exitCode: outcome.exitCode, stdout: toBytes(outcome.stdout), stderr: toBytes(outcome.stderr) };
      } catch (err) {
        result = { exitCode: 1, stdout: new Uint8Array(), stderr: toBytes(`${name}: ${String(err)}\n`) };
      } finally {
        clearTimeout(timer);
      }
    }
    const redactValues = input.redactValues ?? [];
    if (redactValues.length > 0) {
      // Scrub before the result is stored, so a poll never sees the raw value.
      const decoder = new TextDecoder();
      const encoder = new TextEncoder();
      result = {
        ...result,
        stdout: encoder.encode(redactText(decoder.decode(result.stdout), redactValues)),
        stderr: encoder.encode(redactText(decoder.decode(result.stderr), redactValues)),
      };
    }
    if (execution.result === undefined) {
      execution.result = result;
    }
  }

  async pollCommand(leaseId: LeaseId, executionId: string): Promise<PollCommandResponse> {
    const execution = this.getLease(leaseId).executions.get(executionId);
    if (execution === undefined) {
      return { outcome: 'not_found' };
    }
    if (execution.result === undefined) {
      return { outcome: 'running' };
    }
    const { exitCode, stdout, stderr } = execution.result;
    return { outcome: 'completed', exit_code: exitCode, stdout: [...stdout], stderr: [...stderr] };
  }

  async listProcesses(leaseId: LeaseId): Promise<ListProcessesResponse> {
    return {
      outcome: 'processes',
      processes: [...this.getLease(leaseId).executions].map(([execution_id, execution]) => ({
        execution_id,
        ...(execution.input.process?.name === undefined ? {} : { name: execution.input.process.name }),
        ...(execution.result === undefined
          ? { state: 'running' as const }
          : { state: 'exited' as const, exit_code: execution.result.exitCode }),
      })),
    };
  }

  async waitReady(_leaseId: LeaseId, _executionId: string, _timeoutSeconds?: number): Promise<WaitReadyResponse> {
    return { outcome: 'unsupported' };
  }

  async waitSandboxReady(
    _leaseId: LeaseId,
    ports?: number[],
    _timeoutSeconds?: number,
  ): Promise<WaitSandboxReadyResponse> {
    return ports !== undefined && ports.length > 0 ? { outcome: 'unsupported' } : { outcome: 'ready' };
  }

  async killProcess(_leaseId: LeaseId, _name: string, _signal?: UnixSignal): Promise<KillCommandResponse> {
    return { outcome: 'not_found' };
  }

  async killCommand(leaseId: LeaseId, executionId: string, signal: UnixSignal = 'term'): Promise<KillCommandResponse> {
    const execution = this.getLease(leaseId).executions.get(executionId);
    if (execution === undefined) {
      return { outcome: 'not_found' };
    }
    if (execution.result !== undefined) {
      return { outcome: 'already_completed' };
    }
    execution.killed = true;
    execution.result = { exitCode: 128 + SIGNAL_NUMBERS[signal], stdout: new Uint8Array(), stderr: new Uint8Array() };
    execution.abort.abort();
    return { outcome: 'signaled' };
  }

  /**
   * A scripted command cannot ignore SIGTERM, so a stop always ends it with
   * `term` and the grace period is ignored.
   */
  async stopCommand(leaseId: LeaseId, executionId: string, _graceSeconds?: number): Promise<StopCommandResponse> {
    const killed = await this.killCommand(leaseId, executionId, 'term');
    if (killed.outcome === 'signaled') return { outcome: 'stopped', signal: 'term' };
    return { outcome: killed.outcome };
  }

  async restartCommand(leaseId: LeaseId, executionId: string, graceSeconds?: number): Promise<RestartCommandResponse> {
    const previous = this.getLease(leaseId).executions.get(executionId);
    if (previous === undefined) return { outcome: 'not_found' };
    await this.stopCommand(leaseId, executionId, graceSeconds);
    const { command, ...input } = previous.input;
    const launched = await this.launchCommand(leaseId, command, { ...input, detached: true });
    if (launched.outcome !== 'started')
      throw new SandboxWsError('invalid_response', 'a detached process restart did not return an execution');
    return { outcome: 'restarted', execution_id: launched.execution_id };
  }

  async readLogs(
    _leaseId: LeaseId,
    _executionId: string,
    _opts?: { afterSeq?: number; maxBytes?: number },
  ): Promise<ReadLogsResponse> {
    return { outcome: 'unsupported' };
  }

  async *followLogs(
    _leaseId: LeaseId,
    _executionId: string,
    _opts?: { afterSeq?: number },
  ): AsyncGenerator<LogFollowEvent, void, void> {
    yield { type: 'unsupported' };
  }

  async writeFiles(leaseId: LeaseId, files: SandboxFileWrite[]): Promise<WriteFilesResponse> {
    const parsed = parse(writeFilesSchema, files, 'write files');
    const lease = this.getLease(leaseId);
    for (const file of parsed) {
      lease.fs.write(file.path, Uint8Array.from(file.content), { mode: file.mode });
      lease.fileBytesIn += file.content.length;
    }
    return { outcome: 'written' };
  }

  async listDir(leaseId: LeaseId, path: string, opts?: Parameters<SandboxClientApi['listDir']>[2]) {
    const input = parseFileInput(fileListInputSchema, {
      path,
      recursive: opts?.recursive,
      maxEntries: opts?.maxEntries,
      user: opts?.asUser,
    });
    if (input.user !== undefined)
      throw new SandboxWsError('unsupported', 'the memory fake does not model filesystem users');
    return this.getLease(leaseId).fs.listEntries(input.path, input.recursive ?? false, input.maxEntries ?? 1000);
  }

  async editFile(
    leaseId: LeaseId,
    path: string,
    find: string,
    replace: string,
    opts?: Parameters<SandboxClientApi['editFile']>[4],
  ) {
    const input = parseFileInput(fileEditInputSchema, {
      path,
      find,
      replace,
      expectedOccurrences: opts?.expectedOccurrences,
      user: opts?.asUser,
    });
    if (input.user !== undefined)
      throw new SandboxWsError('unsupported', 'the memory fake does not model filesystem users');
    const fs = this.getLease(leaseId).fs;
    if (fs.isDir(input.path)) throw new FileOpFailedError('is_a_directory', 'editing requires a regular file');
    const bytes = fs.read(input.path);
    if (bytes.byteLength > 16 * 1024 * 1024) throw new FileOpFailedError('too_large', 'file exceeds the edit limit');
    let text: string;
    try {
      text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    } catch {
      throw new FileOpFailedError('invalid_request', 'editing requires UTF-8 text');
    }
    const pieces = text.split(input.find);
    const occurrences = pieces.length - 1;
    const expected = input.expectedOccurrences ?? 1;
    if (occurrences !== expected) throw new EditRejectedError(occurrences, expected);
    const result = new TextEncoder().encode(pieces.join(input.replace));
    if (result.byteLength > 16 * 1024 * 1024)
      throw new FileOpFailedError('too_large', 'edited file exceeds the write limit');
    const mode = fs.capture().files.get(fs.resolve(input.path))?.mode;
    fs.write(input.path, result, { mode });
    return { occurrences, size: result.byteLength };
  }

  async searchFiles(leaseId: LeaseId, path: string, opts?: Parameters<SandboxClientApi['searchFiles']>[2]) {
    const input = parseFileInput(
      fileSearchInputSchema.extend({
        query: z
          .string()
          .max(16 * 1024 * 1024)
          .optional(),
      }),
      {
        path,
        query: opts?.pattern,
        glob: opts?.glob,
        regex: opts?.regex,
        caseInsensitive: opts?.caseInsensitive,
        maxResults: opts?.maxResults,
        maxFileBytes: opts?.maxFileBytes,
        user: opts?.asUser,
      },
    );
    if (input.user !== undefined || input.regex === true)
      throw new SandboxWsError('unsupported', 'the memory fake models literal search without filesystem users');
    const fs = this.getLease(leaseId).fs;
    const root = fs.resolve(input.path);
    const files = fs.isDir(root)
      ? fs.tree(root).files
      : [{ path: root.slice(root.lastIndexOf('/') + 1), data: fs.read(root) }];
    const matches: WireSearchMatch[] = [];
    const maxResults = input.maxResults ?? 1000;
    const maxFileBytes = input.maxFileBytes ?? 2 * 1024 * 1024;
    const query = input.caseInsensitive ? input.query?.toLowerCase() : input.query;
    let filesScanned = 0;
    let scannedBytes = 0;
    let truncated = false;
    for (const file of files) {
      const subject = input.glob?.includes('/') ? file.path : file.path.slice(file.path.lastIndexOf('/') + 1);
      if (input.glob !== undefined && !memoryGlobMatch(subject, input.glob)) continue;
      if (query === undefined) {
        if (matches.length >= maxResults) {
          truncated = true;
          break;
        }
        matches.push({ path: file.path });
        continue;
      }
      if (file.data.byteLength > maxFileBytes) continue;
      scannedBytes += file.data.byteLength;
      if (scannedBytes > 256 * 1024 * 1024) {
        truncated = true;
        break;
      }
      filesScanned += 1;
      if (file.data.subarray(0, 8192).includes(0)) continue;
      const lines = new TextDecoder().decode(file.data).split('\n');
      if (lines.at(-1) === '') lines.pop();
      for (let index = 0; index < lines.length; index += 1) {
        const line = lines[index]!.replace(/\r$/, '');
        if (!(input.caseInsensitive ? line.toLowerCase() : line).includes(query)) continue;
        if (matches.length >= maxResults) {
          truncated = true;
          break;
        }
        matches.push({ path: file.path, line: index + 1, text: Array.from(line).slice(0, 500).join('') });
      }
      if (truncated) break;
    }
    return { matches, truncated, filesScanned };
  }

  async readFile(leaseId: LeaseId, path: string): Promise<ReadFileResponse> {
    const lease = this.getLease(leaseId);
    const target = parse(pathSchema, path, 'read file path');
    let data: number[];
    try {
      data = [...lease.fs.read(target)];
    } catch (err) {
      if (err instanceof SandboxWsError && err.code === 'not_found') {
        return { outcome: 'not_found' };
      }
      throw err;
    }
    lease.fileBytesOut += data.length;
    lease.lastActivityMs = this.now();
    return { outcome: 'content', data };
  }

  async stopLease(leaseId: LeaseId): Promise<{ outcome: 'stopped' }> {
    this.release(this.getLease(leaseId));
    return { outcome: 'stopped' };
  }

  async pauseLease(leaseId: LeaseId, maxPausedSeconds?: number): Promise<PauseLeaseResponse> {
    const lease = this.getLease(leaseId);
    const budgetMs = (maxPausedSeconds ?? 3600) * 1000;
    if (lease.pausedRemainingMs === undefined) {
      lease.pausedSinceMs = this.now();
      lease.pausedRemainingMs = Math.max(0, lease.deadlineMs - Date.now());
      lease.deadlineMs = Date.now() + budgetMs;
    }
    return { outcome: 'paused', reap_at: toWireTime(lease.deadlineMs) };
  }

  async resumeLease(leaseId: LeaseId): Promise<ResumeLeaseResponse> {
    const lease = this.getLease(leaseId);
    if (lease.pausedRemainingMs !== undefined) {
      lease.pausedMsTotal += this.now() - (lease.pausedSinceMs ?? this.now());
      lease.pausedSinceMs = undefined;
      lease.deadlineMs = Date.now() + lease.pausedRemainingMs;
      lease.pausedRemainingMs = undefined;
    }
    return { outcome: 'resumed', deadline: toWireTime(lease.deadlineMs) };
  }

  /** The fake suspends to memory only, as the memory worker does. */
  async suspendLease(
    leaseId: LeaseId,
    opts: { level?: SuspendRequestLevel; maxSuspendedSeconds?: number } = {},
  ): Promise<SuspendLeaseResponse> {
    const lease = this.getLease(leaseId);
    if (opts.level === 'disk') {
      throw new SandboxWsError(
        'lease_denied',
        `lease ${leaseId} cannot suspend to level disk: the provider does not offer it`,
        {
          variant: 'suspend_unsupported',
          leaseId,
        },
      );
    }
    if (lease.suspendedRemainingMs === undefined) {
      lease.suspendedSinceMs = this.now();
      lease.suspendedRemainingMs = Math.max(0, lease.deadlineMs - Date.now());
      lease.deadlineMs = Date.now() + (opts.maxSuspendedSeconds ?? 24 * 3600) * 1000;
    }
    return { outcome: 'suspended', level: 'memory', reap_at: toWireTime(lease.deadlineMs) };
  }

  async wakeLease(leaseId: LeaseId): Promise<WakeLeaseResponse> {
    const lease = this.getLease(leaseId);
    if (lease.suspendedRemainingMs !== undefined) {
      lease.suspendedMsTotal += this.now() - (lease.suspendedSinceMs ?? this.now());
      lease.suspendedSinceMs = undefined;
      lease.deadlineMs = Date.now() + lease.suspendedRemainingMs;
      lease.suspendedRemainingMs = undefined;
    }
    return { outcome: 'woken', deadline: toWireTime(lease.deadlineMs) };
  }

  private release(lease: MemoryLease): void {
    if (lease.released) {
      return;
    }
    if (lease.suspendedSinceMs !== undefined) {
      lease.suspendedMsTotal += this.now() - lease.suspendedSinceMs;
      lease.suspendedSinceMs = undefined;
    }
    if (lease.pausedSinceMs !== undefined) {
      lease.pausedMsTotal += this.now() - lease.pausedSinceMs;
      lease.pausedSinceMs = undefined;
    }
    lease.released = true;
    lease.launches.clear();
    lease.endedAtMs = this.now();
  }

  private usageOf(lease: MemoryLease): LeaseUsage {
    const now = lease.endedAtMs ?? this.now();
    const pausedMs = lease.pausedMsTotal + (lease.pausedSinceMs === undefined ? 0 : now - lease.pausedSinceMs);
    const suspendedMs =
      lease.suspendedMsTotal + (lease.suspendedSinceMs === undefined ? 0 : now - lease.suspendedSinceMs);
    const runningMs = Math.max(0, now - lease.startedAtMs - pausedMs - suspendedMs);
    return {
      lease_id: lease.id,
      tenant: lease.tenant,
      status: lease.released
        ? 'released'
        : lease.suspendedSinceMs !== undefined
          ? 'suspended'
          : lease.pausedSinceMs === undefined
            ? 'running'
            : 'paused',
      created_at: toWireTime(lease.startedAtMs),
      ...(lease.endedAtMs === undefined ? {} : { ended_at: toWireTime(lease.endedAtMs) }),
      last_activity_at: toWireTime(lease.lastActivityMs),
      commands_run: lease.commandsRun,
      command_output_bytes: lease.commandOutputBytes,
      file_bytes_in: lease.fileBytesIn,
      file_bytes_out: lease.fileBytesOut,
      running_seconds: Math.floor(runningMs / 1000),
      paused_seconds: Math.floor(pausedMs / 1000),
      suspended_seconds: Math.floor(suspendedMs / 1000),
      // The fake has no runtime to measure, so it reports no gauges.
    };
  }

  /** Whether a released lease's figures have aged out, as the broker's usage store drops them. */
  private usageExpired(lease: MemoryLease): boolean {
    return lease.endedAtMs !== undefined && this.now() - lease.endedAtMs >= USAGE_RETENTION_MS;
  }

  /** Released leases keep returning their final figures for a week after they end, as the broker does. */
  async leaseUsage(leaseId: LeaseId): Promise<LeaseUsage> {
    const lease = this.leases.get(leaseId);
    if (lease === undefined || this.usageExpired(lease)) {
      throw new SandboxWsError('not_found', `lease ${leaseId} not found`);
    }
    return this.usageOf(lease);
  }

  /** Fixed generous limits, with consumption counted from the fake's own leases. */
  async limits(): Promise<AccountLimits> {
    let active = 0;
    for (const lease of this.leases.values()) {
      if (!lease.released) {
        active += 1;
      }
    }
    return {
      active_leases: active,
      max_active_leases: 50,
      cpu_seconds_used: 0,
      cpu_seconds_cap: 3600,
      wall_seconds_used: 0,
      wall_seconds_cap: 7200,
      max_cpu_seconds_per_request: 3600,
      max_memory_mib_per_request: 8192,
    };
  }

  async tenantUsage(): Promise<TenantUsage> {
    const total: TenantUsage = {
      live_leases: 0,
      ended_leases: 0,
      commands_run: 0,
      command_output_bytes: 0,
      file_bytes_in: 0,
      file_bytes_out: 0,
      running_seconds: 0,
      paused_seconds: 0,
    };
    // Released leases keep their final figures in the totals until they age out, as the broker does.
    for (const lease of this.leases.values()) {
      if (this.usageExpired(lease)) {
        continue;
      }
      const u = this.usageOf(lease);
      if (lease.released) {
        total.ended_leases = (total.ended_leases ?? 0) + 1;
      } else {
        total.live_leases += 1;
      }
      total.commands_run += u.commands_run;
      total.command_output_bytes += u.command_output_bytes;
      total.file_bytes_in += u.file_bytes_in;
      total.file_bytes_out += u.file_bytes_out;
      total.running_seconds += u.running_seconds;
      total.paused_seconds += u.paused_seconds;
    }
    return total;
  }

  async deleteLease(leaseId: LeaseId): Promise<{ outcome: 'deleted' }> {
    this.release(this.getLease(leaseId));
    return { outcome: 'deleted' };
  }

  async extendLeaseTimeout(leaseId: LeaseId, additionalSeconds: number): Promise<ExtendLeaseTimeoutResponse> {
    const seconds = parse(durationSchema, additionalSeconds, 'additional seconds');
    const lease = this.getLease(leaseId);
    lease.deadlineMs += seconds * 1000;
    return { outcome: 'extended', new_deadline: toWireTime(lease.deadlineMs) };
  }

  async updateTags(leaseId: LeaseId, tags: Record<string, string>): Promise<{ outcome: 'applied' }> {
    this.getLease(leaseId).tags = { ...parse(z.record(z.string(), z.string()), tags, 'tags') };
    return { outcome: 'applied' };
  }

  /** The in-memory client keeps no persistent entities, so none is ever found. */
  async getEntity(_entity: EntityRef): Promise<GetEntityResponse> {
    return { outcome: 'not_found' };
  }

  async listEntities(_options: ListEntitiesOptions = {}): Promise<ListEntitiesResponse> {
    return { outcome: 'listed', rows: [] };
  }

  async deleteEntity(_entity: EntityRef): Promise<DeleteEntityResponse> {
    return { outcome: 'not_found' };
  }

  async deleteNamedSandbox(name: string): Promise<DeleteNamedSandboxResponse> {
    const lease = [...this.leases.values()].find((l) => !l.released && l.name === name);
    if (lease === undefined) {
      return { outcome: 'not_found' };
    }
    this.release(lease);
    return { outcome: 'deleted' };
  }

  async getLeaseByName(name: string): Promise<GetLeaseByNameResponse> {
    const lease = [...this.leases.values()].find((l) => !l.released && l.name === name);
    if (lease === undefined) {
      return { outcome: 'not_found' };
    }
    return {
      outcome: 'found',
      row: {
        lease_id: lease.id,
        tenant: lease.tenant,
        provider: MEMORY_CAPABILITIES.provider,
        status:
          lease.suspendedRemainingMs !== undefined
            ? 'suspended'
            : lease.pausedRemainingMs !== undefined
              ? 'paused'
              : 'running',
        created_at: toWireTime(lease.startedAtMs),
        deadline: toWireTime(lease.deadlineMs),
        name: lease.name,
        tags: { ...lease.tags },
      },
    };
  }

  async listLeases(opts?: { tags?: Record<string, string> }): Promise<{ leases: LeaseListRow[] }> {
    const wanted = Object.entries(opts?.tags ?? {});
    return {
      leases: [...this.leases.values()]
        .filter((l) => !l.released)
        .filter((l) => wanted.every(([key, value]) => l.tags[key] === value))
        .map((l) => ({
          lease_id: l.id,
          ...(l.name === undefined ? {} : { name: l.name }),
          tenant: l.tenant,
          provider: MEMORY_CAPABILITIES.provider,
          image: { kind: 'registry', reference: { registry: 'memory', repository: 'memory', tag: 'latest' } },
          state:
            l.suspendedRemainingMs !== undefined
              ? 'suspended'
              : l.pausedRemainingMs !== undefined
                ? 'paused'
                : 'running',
          started_at: Math.floor(l.startedAtMs / 1000),
          ttl_seconds: Math.round((l.deadlineMs - l.startedAtMs) / 1000),
          tags: { ...l.tags },
        })),
    };
  }

  async reapLeases(opts: ReapLeasesOptions): Promise<ReapLeasesResponse> {
    if (opts.tags === undefined && opts.older_than_seconds === undefined && opts.idle_for_seconds === undefined) {
      throw new SandboxWsError(
        'invalid_request',
        'reap needs at least one of tags, older_than_seconds, idle_for_seconds',
      );
    }
    const wanted = Object.entries(opts.tags ?? {});
    const now = this.now();
    const matched = [...this.leases.values()]
      .filter((l) => !l.released)
      .filter((l) => wanted.every(([key, value]) => l.tags[key] === value))
      .filter((l) => opts.older_than_seconds === undefined || now - l.startedAtMs >= opts.older_than_seconds * 1000)
      .filter((l) => opts.idle_for_seconds === undefined || now - l.lastActivityMs >= opts.idle_for_seconds * 1000);
    const dryRun = opts.dry_run ?? false;
    if (!dryRun) {
      for (const lease of matched) {
        this.release(lease);
      }
    }
    return {
      outcome: 'reaped',
      dry_run: dryRun,
      matched: matched.map((l) => ({
        lease_id: l.id,
        tenant: l.tenant,
        provider: MEMORY_CAPABILITIES.provider,
        status: 'running',
        tags: { ...l.tags },
      })),
      released: dryRun ? [] : matched.map((l) => l.id),
      failed: [],
    };
  }

  // Verbs below are not modelled. They throw rather than succeed silently.

  fork: SandboxClientApi['fork'] = async () => {
    throw unsupported('fork');
  };
  writeStdin: SandboxClientApi['writeStdin'] = async () => ({ outcome: 'unsupported' });
  openTerminal: SandboxClientApi['openTerminal'] = async () => {
    throw unsupported('terminal');
  };
  terminalInput: SandboxClientApi['terminalInput'] = async () => {
    throw unsupported('terminal');
  };
  terminalResize: SandboxClientApi['terminalResize'] = async () => {
    throw unsupported('terminal');
  };
  terminalClose: SandboxClientApi['terminalClose'] = async () => {
    throw unsupported('terminal');
  };
  updateNetworkPolicy: SandboxClientApi['updateNetworkPolicy'] = async () => {
    throw unsupported('update_network_policy');
  };
  getSandboxDomain: SandboxClientApi['getSandboxDomain'] = async () => ({ outcome: 'unsupported' });
  getNativeHandle: SandboxClientApi['getNativeHandle'] = async () => ({ outcome: 'unsupported' });
  getSshAccess: SandboxClientApi['getSshAccess'] = async () => ({ outcome: 'unsupported' });
  createDrive: SandboxClientApi['createDrive'] = async () => {
    throw unsupported('create_drive');
  };
  listDrives: SandboxClientApi['listDrives'] = async () => {
    throw unsupported('list_drives');
  };
  deleteDrive: SandboxClientApi['deleteDrive'] = async () => {
    throw unsupported('delete_drive');
  };
  createUser: SandboxClientApi['createUser'] = async () => {
    throw unsupported('create_user');
  };
  asUser: SandboxClientApi['asUser'] = () => {
    throw unsupported('as_user');
  };
  createGroup: SandboxClientApi['createGroup'] = async () => {
    throw unsupported('create_group');
  };
  addUserToGroup: SandboxClientApi['addUserToGroup'] = async () => {
    throw unsupported('modify_group_membership');
  };
  removeUserFromGroup: SandboxClientApi['removeUserFromGroup'] = async () => {
    throw unsupported('modify_group_membership');
  };
  evidenceStream: SandboxClientApi['evidenceStream'] = () => {
    throw unsupported('evidence_stream');
  };
  subscribeEvidence: SandboxClientApi['subscribeEvidence'] = () => {
    throw unsupported('subscribe_evidence');
  };
}

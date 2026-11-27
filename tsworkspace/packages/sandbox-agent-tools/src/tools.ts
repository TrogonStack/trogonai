import {
  EditRejectedError,
  FileNotFoundError,
  FileOpUnsupportedError,
  SandboxWsError,
} from '@trogonai/sandbox-ws-client';
import type { Command, CommandFinished, Sandbox, SandboxWsClient, WireSearchMatch } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

import { TOOL_NAMES, toolDescriptions, toolSchemas } from './schemas.js';
import type { ToolInput, ToolName, ToolSchemas } from './schemas.js';

/** The slice of `Sandbox` the tools use, so a test double or wrapper can stand in. */
export type ToolSandbox = Pick<Sandbox, 'sandboxId' | 'runCommand' | 'readFileToBuffer' | 'writeFiles' | 'domain'> &
  Partial<Pick<Sandbox, 'startServer'>> & { fs?: Pick<Sandbox['fs'], 'checkpoint' | 'changes' | 'watch'> };

/** Structured file verbs. When present they replace the shell and read-modify-write fallbacks. */
export interface NativeFileOps {
  edit(path: string, find: string, replace: string, expectedOccurrences: number): Promise<{ occurrences: number }>;
  search(
    path: string,
    opts: { pattern: string; glob?: string; maxResults: number },
  ): Promise<{ matches: WireSearchMatch[]; truncated: boolean }>;
  list(
    path: string,
    opts: { recursive: boolean; maxEntries: number },
  ): Promise<{ entries: { path: string; kind: string }[]; truncated: boolean }>;
}

/** Binds the gateway client's `file_op` helpers to one lease. */
export function nativeFileOps(
  client: Pick<SandboxWsClient, 'editFile' | 'searchFiles' | 'listDir'>,
  sandboxId: Parameters<SandboxWsClient['listDir']>[0],
): NativeFileOps {
  return {
    async edit(path, find, replace, expectedOccurrences) {
      const { occurrences } = await client.editFile(sandboxId, path, find, replace, { expectedOccurrences });
      return { occurrences };
    },
    async search(path, opts) {
      const { matches, truncated } = await client.searchFiles(sandboxId, path, {
        pattern: opts.pattern,
        glob: opts.glob,
        maxResults: opts.maxResults,
      });
      return { matches, truncated };
    },
    async list(path, opts) {
      const { entries, truncated } = await client.listDir(sandboxId, path, {
        recursive: opts.recursive,
        maxEntries: opts.maxEntries,
      });
      return { entries: entries.map((e) => ({ path: e.path, kind: e.kind })), truncated };
    },
  };
}

export interface SandboxToolsOptions {
  /** Offer only these tools. Defaults to all of them. */
  allow?: readonly ToolName[];
  /** Never offer these tools. Wins over `allow`. */
  deny?: readonly ToolName[];
  /**
   * Host-side policy hook run after input validation and before a tool executes.
   * Return `{ deny }` to refuse the call (the model sees a `refused` verdict with
   * the reason) or `{ command }` to replace the shell command of `run_command` and
   * `background_process` start. A throw is treated as a refusal. Set by the host,
   * never by the model.
   */
  beforeRun?: BeforeRun;
  /** Working directory for commands and the base for relative paths. */
  cwd?: string;
  /** Timeout applied when a call gives none. Defaults to 60. */
  defaultTimeoutSeconds?: number;
  /** Upper bound on any requested timeout. Defaults to 600. */
  maxTimeoutSeconds?: number;
  /** Per-stream cap asked of the stack (`max_output_bytes`). The stack keeps head and tail. Defaults to 64 KiB. */
  maxOutputBytes?: number;
  /** Cap on characters returned to the model per section. Defaults to 30000. */
  maxOutputChars?: number;
  /**
   * Exact secret values the stack scrubs from command output (`redact_values`). Set by the
   * host, never by the model. Needs a provider that supports output redaction; encoded
   * copies of a value are not caught.
   */
  redactValues?: readonly string[];
  /** Lines `read_file` returns when no limit is given. Defaults to 2000. */
  defaultReadLines?: number;
  /** Structured file verbs; see {@link nativeFileOps}. */
  nativeFileOps?: NativeFileOps;
  /** Upper bound on `watch_files` duration. Defaults to 30 seconds. */
  maxWatchSeconds?: number;
  /** Minimum gap between progress snapshots of a streamed `run_command`. Defaults to 250 ms. */
  streamIntervalMs?: number;
  /** Scheme put in front of the endpoint `preview_url` resolves. Defaults to `http`. */
  previewScheme?: 'http' | 'https';
}

export interface BeforeRunCall {
  readonly tool: ToolName;
  /** The validated input the tool would run with. */
  readonly input: Readonly<Record<string, unknown>>;
  /** The shell command line, for tools that run one. */
  readonly command?: string;
}

export type BeforeRunDecision = void | undefined | { deny: string } | { command: string };

export type BeforeRun = (call: BeforeRunCall) => BeforeRunDecision | Promise<BeforeRunDecision>;

export interface SandboxTool<N extends ToolName = ToolName> {
  readonly name: N;
  readonly description: string;
  readonly inputSchema: ToolSchemas[N];
  /**
   * Validates raw model input, runs the tool and returns text. Never throws for
   * bad input or a failed operation: the text starts with a `Verdict:` line.
   */
  execute(input: unknown): Promise<string>;
  /**
   * Present on tools that can report progress, which is `run_command`. Yields text snapshots
   * while the work runs; the last value is the same text `execute` would return. Validation
   * and policy run first, and a refusal or error is a single value. Falls back to one value
   * when the stack keeps no durable command logs.
   */
  stream?(input: unknown): AsyncGenerator<string, void, void>;
}

export type SandboxToolSet = { [N in ToolName]?: SandboxTool<N> };

interface Resolved {
  cwd: string | undefined;
  defaultTimeoutSeconds: number;
  maxTimeoutSeconds: number;
  maxOutputBytes: number;
  maxOutputChars: number;
  defaultReadLines: number;
  redactValues: string[] | undefined;
  maxWatchSeconds: number;
  streamIntervalMs: number;
}

/** Keeps the head and tail of an oversize string and drops the middle. */
export function clip(text: string, maxChars: number): { text: string; clipped: boolean } {
  if (text.length <= maxChars) {
    return { text, clipped: false };
  }
  const head = Math.ceil(maxChars * 0.6);
  const tail = maxChars - head;
  const dropped = text.length - head - tail;
  return {
    text: `${text.slice(0, head)}\n... [${dropped} characters omitted] ...\n${text.slice(text.length - tail)}`,
    clipped: true,
  };
}

function verdict(line: string, ...body: string[]): string {
  const rest = body.filter((part) => part.length > 0);
  return [`Verdict: ${line}`, ...rest].join('\n');
}

function describeError(err: unknown): string {
  if (err instanceof z.ZodError) {
    return `invalid input: ${err.issues.map((i) => `${i.path.join('.') || 'input'} ${i.message}`).join('; ')}`;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return String(err);
}

function resolvePath(cwd: string | undefined, path: string): string {
  if (path.startsWith('/') || cwd === undefined) {
    return path;
  }
  return `${cwd.replace(/\/+$/, '')}/${path}`;
}

function section(label: string, text: string, maxChars: number): string {
  if (text.length === 0) {
    return '';
  }
  return `${label}:\n${clip(text, maxChars).text}`;
}

function countOccurrences(haystack: string, needle: string): number {
  let count = 0;
  for (let at = haystack.indexOf(needle); at !== -1; at = haystack.indexOf(needle, at + needle.length)) {
    count += 1;
  }
  return count;
}

function isBinary(bytes: Uint8Array): boolean {
  return bytes.subarray(0, 8000).includes(0);
}

function dirname(path: string): string {
  const cut = path.lastIndexOf('/');
  return cut <= 0 ? '/' : path.slice(0, cut);
}

function commandVerdict(result: CommandFinished, timeoutSeconds: number): string {
  if (result.timedOut) {
    return `timed out after ${timeoutSeconds}s and was killed`;
  }
  return result.exitCode === 0 ? 'ok (exit 0)' : `failed (exit ${result.exitCode})`;
}

function formatFinished(result: CommandFinished, verdictLine: string, maxChars: number): string {
  const notes: string[] = [];
  if (result.truncated) {
    notes.push('Note: output exceeded the retention cap; the middle was dropped.');
  }
  const stdout = result.stdout();
  const stderr = result.stderr();
  if (stdout.includes('�') || stderr.includes('�')) {
    notes.push('Note: output contained bytes that are not valid UTF-8 and were replaced.');
  }
  return verdict(verdictLine, ...notes, section('stdout', stdout, maxChars), section('stderr', stderr, maxChars));
}

/**
 * Builds the tool bundle for one sandbox. The result is framework neutral:
 * each tool carries a zod input schema, a description and an `execute` that
 * returns plain text. Use the entry points under `./ai-sdk`, `./mastra` and
 * `./openai-agents` to hand them to a framework.
 */
export function createSandboxTools(sandbox: ToolSandbox, options: SandboxToolsOptions = {}): SandboxToolSet {
  const cfg: Resolved = {
    cwd: options.cwd,
    defaultTimeoutSeconds: options.defaultTimeoutSeconds ?? 60,
    maxTimeoutSeconds: options.maxTimeoutSeconds ?? 600,
    maxOutputBytes: options.maxOutputBytes ?? 64 * 1024,
    maxOutputChars: options.maxOutputChars ?? 30_000,
    defaultReadLines: options.defaultReadLines ?? 2000,
    redactValues:
      options.redactValues === undefined || options.redactValues.length === 0 ? undefined : [...options.redactValues],
    maxWatchSeconds: options.maxWatchSeconds ?? 30,
    streamIntervalMs: options.streamIntervalMs ?? 250,
  };
  const native = options.nativeFileOps;
  /** The command is read on each use: a server restarted on resume swaps its command. */
  const processes = new Map<string, { readonly command: Command; stop?(): Promise<void> }>();

  async function shell(command: string, cwd: string | undefined, timeoutSeconds: number): Promise<CommandFinished> {
    return sandbox.runCommand({
      cmd: 'sh',
      args: ['-c', command],
      cwd,
      timeoutSeconds,
      maxOutputBytes: cfg.maxOutputBytes,
      redactValues: cfg.redactValues,
    });
  }

  const impl: { [N in ToolName]: (input: ToolInput<N>) => Promise<string> } = {
    async run_command(input) {
      const timeout = Math.min(input.timeout_seconds ?? cfg.defaultTimeoutSeconds, cfg.maxTimeoutSeconds);
      const result = await shell(
        input.command,
        input.cwd === undefined ? cfg.cwd : resolvePath(cfg.cwd, input.cwd),
        timeout,
      );
      return formatFinished(result, commandVerdict(result, timeout), cfg.maxOutputChars);
    },

    async read_file(input) {
      const path = resolvePath(cfg.cwd, input.path);
      const bytes = await sandbox.readFileToBuffer({ path });
      if (bytes === null) {
        return verdict(`error, no such file: ${path}`);
      }
      if (isBinary(bytes)) {
        return verdict(
          `error, ${path} looks binary (${bytes.length} bytes); use run_command with a tool such as file or xxd`,
        );
      }
      const lines = new TextDecoder().decode(bytes).split('\n');
      if (lines.at(-1) === '') {
        lines.pop();
      }
      const start = (input.offset ?? 1) - 1;
      const limit = input.limit ?? cfg.defaultReadLines;
      const page = lines.slice(start, start + limit);
      if (start >= lines.length && lines.length > 0) {
        return verdict(`error, offset ${start + 1} is past the end of ${path} (${lines.length} lines)`);
      }
      const numbered = page.map((line, i) => `${start + i + 1}\t${line}`).join('\n');
      const more = start + page.length < lines.length;
      const clipped = clip(numbered, cfg.maxOutputChars);
      const head = `ok, ${path} lines ${page.length === 0 ? 0 : start + 1}-${start + page.length} of ${lines.length}`;
      const hint = more ? `Note: more lines remain; call again with offset ${start + page.length + 1}.` : '';
      const clipNote = clipped.clipped ? 'Note: output was clipped to fit; request a smaller limit.' : '';
      return verdict(head, hint, clipNote, clipped.text);
    },

    async write_file(input) {
      const path = resolvePath(cfg.cwd, input.path);
      const parent = dirname(path);
      if (parent !== '/') {
        const made = await sandbox.runCommand({ cmd: 'mkdir', args: ['-p', parent] });
        if (made.exitCode !== 0) {
          return verdict(`error, could not create ${parent}: ${made.stderr().trim()}`);
        }
      }
      await sandbox.writeFiles([{ path, content: input.content }]);
      return verdict(`ok, wrote ${new TextEncoder().encode(input.content).length} bytes to ${path}`);
    },

    async edit_file(input) {
      const path = resolvePath(cfg.cwd, input.path);
      const expected = input.expected_occurrences ?? 1;
      if (native !== undefined) {
        try {
          const { occurrences } = await native.edit(path, input.old_text, input.new_text, expected);
          return verdict(`ok, replaced ${occurrences} occurrence(s) in ${path}`);
        } catch (err) {
          if (err instanceof EditRejectedError) {
            return verdict(
              `refused, old_text occurs ${err.occurrences} time(s) but ${err.expected} expected; nothing was written`,
            );
          }
          if (err instanceof FileNotFoundError) {
            return verdict(`error, no such file: ${path}`);
          }
          if (!(err instanceof FileOpUnsupportedError)) {
            throw err;
          }
        }
      }
      const bytes = await sandbox.readFileToBuffer({ path });
      if (bytes === null) {
        return verdict(`error, no such file: ${path}`);
      }
      const text = new TextDecoder().decode(bytes);
      const found = countOccurrences(text, input.old_text);
      if (found !== expected) {
        return verdict(`refused, old_text occurs ${found} time(s) but ${expected} expected; nothing was written`);
      }
      await sandbox.writeFiles([{ path, content: text.split(input.old_text).join(input.new_text) }]);
      return verdict(`ok, replaced ${found} occurrence(s) in ${path}`);
    },

    async find_files(input) {
      const path = resolvePath(cfg.cwd, input.path ?? '.');
      const max = input.max_results ?? 100;
      if (input.pattern !== undefined) {
        return searchContents(path, input.pattern, input.glob, max);
      }
      return listEntries(path, input.glob, input.recursive ?? false, max);
    },

    async background_process(input) {
      const existing = processes.get(input.name);
      if (input.action === 'start') {
        if (input.command === undefined || input.command.length === 0) {
          return verdict('error, `command` is required to start a process');
        }
        if (existing !== undefined && (await pollProcess(existing.command)).running) {
          return verdict(`error, process ${input.name} is already running; stop it first`);
        }
        const started = await sandbox.runCommand({
          cmd: 'sh',
          args: ['-c', input.command],
          cwd: input.cwd === undefined ? cfg.cwd : resolvePath(cfg.cwd, input.cwd),
          detached: true,
          deadline: 'lease',
          maxOutputBytes: cfg.maxOutputBytes,
          redactValues: cfg.redactValues,
        });
        processes.set(input.name, { command: started });
        return verdict(
          `ok, started ${input.name}`,
          'Use action `status` to check on it. Output is reported once it exits.',
        );
      }
      if (existing === undefined) {
        return verdict(`error, no process named ${input.name} was started through this bundle`);
      }
      if (input.action === 'stop') {
        const state = await pollProcess(existing.command);
        if (!state.running) {
          return processReport(input.name, state.finished!);
        }
        // A server also leaves the sandbox's restart-on-resume list.
        await (existing.stop === undefined ? existing.command.kill('term') : existing.stop());
        return verdict(`ok, sent SIGTERM to ${input.name}`);
      }
      const state = await pollProcess(existing.command);
      return state.running ? verdict(`ok, ${input.name} is running`) : processReport(input.name, state.finished!);
    },

    async start_server(input) {
      if (sandbox.startServer === undefined) {
        return verdict('error, this sandbox cannot start servers');
      }
      const existing = processes.get(input.name);
      if (existing !== undefined && (await pollProcess(existing.command)).running) {
        return verdict(`error, process ${input.name} is already running; stop it first`);
      }
      const timeoutSeconds = Math.min(input.timeout_seconds ?? 60, cfg.maxTimeoutSeconds);
      const server = await sandbox.startServer({
        cmd: 'sh',
        args: ['-c', input.command],
        cwd: input.cwd === undefined ? cfg.cwd : resolvePath(cfg.cwd, input.cwd),
        port: input.port,
        path: input.path,
        timeoutMs: timeoutSeconds * 1000,
        maxOutputBytes: cfg.maxOutputBytes,
        redactValues: cfg.redactValues,
      });
      processes.set(input.name, server);
      const where = input.path === undefined ? `port ${input.port}` : `${input.path} on port ${input.port}`;
      return verdict(`ok, ${input.name} is ready (${where} answers)`, server.url ?? '');
    },

    async preview_url(input) {
      try {
        const endpoint = await sandbox.domain(input.port);
        const url = /^[a-z]+:\/\//.test(endpoint) ? endpoint : `${options.previewScheme ?? 'http'}://${endpoint}`;
        return verdict(`ok, port ${input.port} is reachable`, url);
      } catch (err) {
        return verdict(`error, ${describeError(err)}`);
      }
    },

    async checkpoint_files(input) {
      if (sandbox.fs === undefined) {
        return verdict('error, this sandbox cannot record checkpoints');
      }
      const path = resolvePath(cfg.cwd, input.path ?? '.');
      const { checkpointId, entries } = await sandbox.fs.checkpoint(path, { hash: input.hash });
      return verdict(`ok, recorded ${entries} entr(ies) under ${path}`, `checkpoint_id: ${checkpointId}`);
    },

    async file_changes(input) {
      if (sandbox.fs === undefined) {
        return verdict('error, this sandbox cannot report file changes');
      }
      const set = await sandbox.fs.changes({
        since: input.since,
        path: input.path === undefined ? undefined : resolvePath(cfg.cwd, input.path),
      });
      const total = set.added.length + set.modified.length + set.deleted.length;
      const lines = [
        ...set.added.map((p) => `added ${p}`),
        ...set.modified.map((p) => `modified ${p}`),
        ...set.deleted.map((p) => `deleted ${p}`),
      ];
      const note = set.truncated ? 'Note: the lists were cut at the sandbox limit; narrow the path.' : '';
      const head = total === 0 ? `ok, no changes since ${set.since}` : `ok, ${total} change(s) since ${set.since}`;
      return verdict(head, note, clip(lines.join('\n'), cfg.maxOutputChars).text);
    },

    async watch_files(input) {
      if (sandbox.fs === undefined) {
        return verdict('error, this sandbox cannot watch files');
      }
      const path = resolvePath(cfg.cwd, input.path);
      const seconds = Math.min(input.duration_seconds ?? 10, cfg.maxWatchSeconds);
      const maxEvents = input.max_events ?? 50;
      const abort = new AbortController();
      const lines: string[] = [];
      let overflowed = false;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const timeUp = new Promise<'time'>((resolve) => {
        timer = setTimeout(() => resolve('time'), seconds * 1000);
      });
      const iterator = sandbox.fs.watch(path, { recursive: input.recursive ?? false, signal: abort.signal });
      let ended: 'time' | 'events' | 'closed' = 'closed';
      try {
        for (;;) {
          const next = await Promise.race([iterator.next(), timeUp]);
          if (next === 'time') {
            ended = 'time';
            break;
          }
          if (next.done === true) {
            break;
          }
          if (next.value.type === 'overflowed') {
            overflowed = true;
          } else {
            lines.push(`${next.value.event.kind} ${next.value.event.path}`);
          }
          if (lines.length >= maxEvents) {
            ended = 'events';
            break;
          }
        }
      } finally {
        clearTimeout(timer);
        abort.abort();
        void iterator.return().catch(() => undefined);
      }
      const why =
        ended === 'time'
          ? `after ${seconds}s`
          : ended === 'events'
            ? `at the ${maxEvents} event limit`
            : 'when the watch closed';
      const head =
        lines.length === 0 ? `ok, no events under ${path} ${why}` : `ok, ${lines.length} event(s) under ${path} ${why}`;
      const note = overflowed
        ? 'Note: some events were dropped before they were read; use file_changes for the net result.'
        : '';
      return verdict(head, note, clip(lines.join('\n'), cfg.maxOutputChars).text);
    },
  };

  async function pollProcess(
    command: Command,
  ): Promise<{ running: true; finished?: undefined } | { running: false; finished: CommandFinished }> {
    try {
      return { running: false, finished: await command.wait({ timeoutMs: 0 }) };
    } catch (err) {
      if (err instanceof Error && err.message.startsWith('timed out waiting')) {
        return { running: true };
      }
      throw err;
    }
  }

  function processReport(name: string, finished: CommandFinished): string {
    return formatFinished(finished, `${name} exited (exit ${finished.exitCode})`, cfg.maxOutputChars);
  }

  async function searchContents(path: string, pattern: string, glob: string | undefined, max: number): Promise<string> {
    if (native !== undefined) {
      try {
        const { matches, truncated } = await native.search(path, { pattern, glob, maxResults: max });
        const lines = matches.map((m) => (m.line === undefined ? m.path : `${m.path}:${m.line}: ${m.text ?? ''}`));
        return searchReport(path, lines, truncated, max);
      } catch (err) {
        if (!(err instanceof FileOpUnsupportedError)) {
          throw err;
        }
      }
    }
    const args = ['-rnIF', ...(glob === undefined ? [] : ['--include', glob]), '--', pattern, path];
    const result = await sandbox.runCommand({ cmd: 'grep', args, maxOutputBytes: cfg.maxOutputBytes });
    if (result.exitCode > 1) {
      return verdict(
        `error, search failed (exit ${result.exitCode})`,
        section('stderr', result.stderr(), cfg.maxOutputChars),
      );
    }
    const lines = result
      .stdout()
      .split('\n')
      .filter((l) => l.length > 0);
    return searchReport(path, lines.slice(0, max), lines.length > max || result.truncated, max);
  }

  function searchReport(path: string, lines: string[], truncated: boolean, max: number): string {
    const head = lines.length === 0 ? `ok, no matches under ${path}` : `ok, ${lines.length} match(es) under ${path}`;
    const note = truncated ? `Note: results capped at ${max}; narrow the path or glob.` : '';
    return verdict(head, note, clip(lines.join('\n'), cfg.maxOutputChars).text);
  }

  async function listEntries(path: string, glob: string | undefined, recursive: boolean, max: number): Promise<string> {
    if (native !== undefined) {
      try {
        const { entries, truncated } = await native.list(path, { recursive, maxEntries: max });
        const lines = entries.map((e) => (e.kind === 'directory' ? `${e.path}/` : e.path));
        return listReport(path, lines, truncated, max);
      } catch (err) {
        if (err instanceof FileNotFoundError) {
          return verdict(`error, no such directory: ${path}`);
        }
        if (!(err instanceof FileOpUnsupportedError)) {
          throw err;
        }
      }
    }
    const result =
      recursive || glob !== undefined
        ? await sandbox.runCommand({
            cmd: 'find',
            args: [
              path,
              ...(recursive ? [] : ['-maxdepth', '1']),
              '-type',
              'f',
              ...(glob === undefined ? [] : ['-name', glob]),
            ],
            maxOutputBytes: cfg.maxOutputBytes,
          })
        : await sandbox.runCommand({ cmd: 'ls', args: ['-A', '-1', path], maxOutputBytes: cfg.maxOutputBytes });
    if (result.exitCode !== 0) {
      return verdict(
        `error, could not list ${path} (exit ${result.exitCode})`,
        section('stderr', result.stderr(), cfg.maxOutputChars),
      );
    }
    const lines = result
      .stdout()
      .split('\n')
      .filter((l) => l.length > 0);
    return listReport(path, lines.slice(0, max), lines.length > max || result.truncated, max);
  }

  function listReport(path: string, lines: string[], truncated: boolean, max: number): string {
    const head =
      lines.length === 0 ? `ok, ${path} has no matching entries` : `ok, ${lines.length} entr(ies) in ${path}`;
    const note = truncated ? `Note: results capped at ${max}; narrow the path or glob.` : '';
    return verdict(head, note, clip(lines.join('\n'), cfg.maxOutputChars).text);
  }

  /** Validates and applies the host policy. Returns the data to run with, or a finished verdict. */
  async function prepare(
    name: ToolName,
    input: unknown,
  ): Promise<{ data: Record<string, unknown> } | { text: string }> {
    const parsed = toolSchemas[name].safeParse(input);
    if (!parsed.success) {
      return { text: verdict(`error, ${describeError(parsed.error)}`) };
    }
    let data: Record<string, unknown> = parsed.data;
    if (options.beforeRun !== undefined) {
      const command = typeof data.command === 'string' ? data.command : undefined;
      let decision: BeforeRunDecision;
      try {
        decision = await options.beforeRun({ tool: name, input: data, command });
      } catch (err) {
        return { text: verdict(`refused, policy hook failed: ${describeError(err)}`) };
      }
      if (decision !== undefined && 'deny' in decision) {
        return { text: verdict(`refused, ${decision.deny}`) };
      }
      if (decision !== undefined && 'command' in decision) {
        if (command === undefined) {
          return { text: verdict(`refused, ${name} has no command to rewrite`) };
        }
        data = { ...data, command: decision.command };
      }
    }
    return { data };
  }

  function failure(err: unknown): string {
    const prefix = err instanceof SandboxWsError ? `${err.code}: ` : '';
    return verdict(`error, ${prefix}${describeError(err)}`);
  }

  /**
   * The command's output as it is produced: the durable log when the stack keeps one, else the
   * live chunk feed until the command exits. Live chunks written before the feed attached are
   * not replayed; the final result still has them.
   */
  async function* followOutput(
    command: Command,
  ): AsyncGenerator<{ stream: 'stdout' | 'stderr'; data: string }, void, void> {
    try {
      yield* command.logs({ follow: true });
      return;
    } catch (err) {
      if (!(err instanceof SandboxWsError && err.code === 'unsupported')) {
        return;
      }
    }
    const decoders = { stdout: new TextDecoder(), stderr: new TextDecoder() };
    const exited = command.wait().then(
      () => 'exited' as const,
      () => 'exited' as const,
    );
    const live = command.stream();
    try {
      for (;;) {
        const next = await Promise.race([live.next(), exited]);
        if (next === 'exited' || next.done === true) {
          return;
        }
        yield {
          stream: next.value.stream,
          data: decoders[next.value.stream].decode(next.value.data, { stream: true }),
        };
      }
    } finally {
      void live.return().catch(() => undefined);
    }
  }

  /** `run_command` that reports progress from the command's streamed output while it runs. */
  async function* streamRunCommand(input: ToolInput<'run_command'>): AsyncGenerator<string, void, void> {
    const timeout = Math.min(input.timeout_seconds ?? cfg.defaultTimeoutSeconds, cfg.maxTimeoutSeconds);
    const command = await sandbox.runCommand({
      cmd: 'sh',
      args: ['-c', input.command],
      cwd: input.cwd === undefined ? cfg.cwd : resolvePath(cfg.cwd, input.cwd),
      timeoutSeconds: timeout,
      maxOutputBytes: cfg.maxOutputBytes,
      redactValues: cfg.redactValues,
      detached: true,
      // Workers publish chunks only when asked; both the durable log and the live feed need them.
      streamOutput: true,
    });
    let out = '';
    let err = '';
    let lastYield = 0;
    try {
      for await (const chunk of followOutput(command)) {
        if (chunk.stream === 'stdout') {
          out += chunk.data;
        } else {
          err += chunk.data;
        }
        const now = Date.now();
        if (now - lastYield >= cfg.streamIntervalMs) {
          lastYield = now;
          yield verdict(
            'running',
            section('stdout so far', out, cfg.maxOutputChars),
            section('stderr so far', err, cfg.maxOutputChars),
          );
        }
      }
    } catch {
      // The feed dropped: the final result below is still exact.
    }
    const result = await command.wait();
    yield formatFinished(result, commandVerdict(result, timeout), cfg.maxOutputChars);
  }

  const denied = new Set<ToolName>(options.deny ?? []);
  const allowed = new Set<ToolName>(options.allow ?? TOOL_NAMES);
  const set: Record<string, SandboxTool> = {};
  for (const name of TOOL_NAMES) {
    if (!allowed.has(name) || denied.has(name)) {
      continue;
    }
    const run = impl[name] as (input: unknown) => Promise<string>;
    const tool: SandboxTool = {
      name,
      description: toolDescriptions[name],
      inputSchema: toolSchemas[name],
      async execute(input: unknown): Promise<string> {
        const prepared = await prepare(name, input);
        if ('text' in prepared) {
          return prepared.text;
        }
        try {
          return await run(prepared.data as never);
        } catch (err) {
          return failure(err);
        }
      },
    };
    if (name === 'run_command') {
      tool.stream = async function* (input: unknown): AsyncGenerator<string, void, void> {
        const prepared = await prepare(name, input);
        if ('text' in prepared) {
          yield prepared.text;
          return;
        }
        try {
          yield* streamRunCommand(prepared.data as ToolInput<'run_command'>);
        } catch (err) {
          yield failure(err);
        }
      };
    }
    set[name] = tool;
  }
  return set as SandboxToolSet;
}

/** The tools in a set as an array, in the order models see them. */
export function toolList(set: SandboxToolSet): SandboxTool[] {
  return TOOL_NAMES.flatMap((name) => {
    const tool = set[name];
    return tool === undefined ? [] : [tool as SandboxTool];
  });
}

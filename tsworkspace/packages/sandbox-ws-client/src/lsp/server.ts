import type { SandboxClientApi } from '../client-api.ts';
import type { EvidenceSubscription } from '../client.ts';
import type { LeaseId } from '../ids.ts';
import type { Command, Sandbox } from '../sandbox.ts';
import type { CommandOutputChunk } from '../wire.ts';
import { LspNotRunningError, LspProtocolError, LspServerExitedError } from './errors.ts';
import { encodeMessage, LspFramer } from './framer.ts';
import { JsonRpcConnection } from './jsonrpc.ts';
import type { NotificationHandler } from './jsonrpc.ts';
import { basenamePosix, fileUri, isAbsolutePosix, resolvePosix } from './path.ts';
import type {
  CompletionItem,
  CompletionList,
  ConfigurationItem,
  DocumentSymbol,
  Hover,
  InitializeResult,
  Location,
  LocationLink,
  Position,
  SymbolInformation,
} from './types.ts';

const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;
const DEFAULT_STOP_TIMEOUT_MS = 5000;
const DEFAULT_KILL_GRACE_MS = 2000;
const DEFAULT_EXIT_POLL_INTERVAL_MS = 1000;
const EXIT_POLL_MAX_CONSECUTIVE_FAILURES = 5;
const STOP_POLL_INTERVAL_MS = 50;

export interface LspServerSpec {
  command: string;
  args?: string[];
  env?: Record<string, string>;
  languageId?: string;
  initializationOptions?: unknown;
}

export interface LspServerOptions {
  requestTimeoutMs?: number;
  /** Time to wait for the server to exit on its own before it is signalled. */
  stopTimeoutMs?: number;
  /** Time to wait after SIGTERM before SIGKILL. */
  killGraceMs?: number;
  /** How often the process is polled to detect that it exited. */
  exitPollIntervalMs?: number;
  /** Merged over the default client capabilities. */
  capabilities?: Record<string, unknown>;
  /** Answers `workspace/configuration` items. Defaults to `null` for every item. */
  configuration?: (item: ConfigurationItem) => unknown;
}

type LspState = 'idle' | 'starting' | 'running' | 'stopping' | 'stopped';

const DEFAULT_CLIENT_CAPABILITIES = {
  workspace: { configuration: true, workspaceFolders: true },
  textDocument: {
    synchronization: { didSave: false, dynamicRegistration: false },
    documentSymbol: { hierarchicalDocumentSymbolSupport: true },
    completion: { completionItem: { snippetSupport: false, documentationFormat: ['markdown', 'plaintext'] } },
    hover: { contentFormat: ['markdown', 'plaintext'] },
    definition: { linkSupport: true },
    publishDiagnostics: {},
  },
};

export class LspServer {
  private state: LspState = 'idle';
  private startPromise: Promise<InitializeResult> | undefined;
  private stopPromise: Promise<void> | undefined;
  private command: Command | undefined;
  private subscription: EvidenceSubscription | undefined;
  private connection: JsonRpcConnection | undefined;
  private readonly documents = new Map<string, number>();
  private readonly stderrHandlers = new Set<(text: string) => void>();
  private readonly notificationHandlers: Array<[string, NotificationHandler]> = [];
  private readonly projectDir: string;
  private watchTimer: ReturnType<typeof setTimeout> | undefined;
  private watchWake: (() => void) | undefined;

  constructor(
    private readonly client: SandboxClientApi,
    private readonly sandbox: Pick<Sandbox, 'sandboxId' | 'runCommand'>,
    private readonly spec: LspServerSpec,
    projectDir: string,
    private readonly opts: LspServerOptions = {},
  ) {
    if (!isAbsolutePosix(projectDir)) {
      throw new Error(`projectDir must be an absolute path, got "${projectDir}"`);
    }
    this.projectDir = resolvePosix(projectDir, '.');
  }

  get leaseId(): LeaseId {
    return this.sandbox.sandboxId;
  }

  uri(path: string): string {
    return fileUri(resolvePosix(this.projectDir, path));
  }

  start(): Promise<InitializeResult> {
    if (this.stopPromise) {
      return Promise.reject(new LspNotRunningError('language server was stopped'));
    }
    this.startPromise ??= this.doStart();
    return this.startPromise;
  }

  private async doStart(): Promise<InitializeResult> {
    this.state = 'starting';
    try {
      const subscription = this.client.subscribeEvidence(this.leaseId);
      this.subscription = subscription;
      await subscription.ready;
      const command = (await this.sandbox.runCommand({
        cmd: this.spec.command,
        args: this.spec.args,
        cwd: this.projectDir,
        env: this.spec.env,
        detached: true,
        streamOutput: true,
        stdin: 'interactive',
        outputRetention: 'stream_only',
        deadline: 'lease',
      })) as Command;
      this.command = command;
      const connection = new JsonRpcConnection({
        send: (message) => command.writeStdin(encodeMessage(message)),
        requestTimeoutMs: this.opts.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS,
        configuration: this.opts.configuration,
      });
      for (const [method, handler] of this.notificationHandlers) {
        connection.onNotification(method, handler);
      }
      this.connection = connection;
      void this.pump(subscription, command, connection);
      void this.watchExit(command);
      const result = await connection.request<InitializeResult>('initialize', {
        processId: null,
        rootUri: this.uri('.'),
        workspaceFolders: [{ uri: this.uri('.'), name: basenamePosix(this.projectDir) || '/' }],
        capabilities: mergeCapabilities(DEFAULT_CLIENT_CAPABILITIES, this.opts.capabilities),
        initializationOptions: this.spec.initializationOptions,
      });
      await connection.notify('initialized', {});
      if (this.state === 'starting') {
        this.state = 'running';
      }
      return result;
    } catch (err) {
      await this.command?.kill('kill').catch(() => undefined);
      await this.teardown();
      throw err;
    }
  }

  private async pump(
    subscription: EvidenceSubscription,
    command: Command,
    connection: JsonRpcConnection,
  ): Promise<void> {
    const suffix = `command.${command.cmdId}`;
    const framer = new LspFramer();
    const stderrDecoder = new TextDecoder();
    let lastStdoutSeq = -1;
    let lastStderrSeq = -1;
    try {
      for await (const event of subscription.events) {
        if (event.subjectSuffix !== suffix) {
          continue;
        }
        const text = typeof event.payload === 'string' ? event.payload : new TextDecoder().decode(event.payload);
        const chunk = JSON.parse(text) as CommandOutputChunk;
        const data = Uint8Array.from(chunk.data);
        if (chunk.stream === 'stderr') {
          if (chunk.seq <= lastStderrSeq) {
            continue;
          }
          lastStderrSeq = chunk.seq;
          const decoded = stderrDecoder.decode(data, { stream: true });
          for (const handler of this.stderrHandlers) {
            try {
              handler(decoded);
            } catch {
              // a throwing listener must not break the output pump
            }
          }
          continue;
        }
        if (chunk.seq <= lastStdoutSeq) {
          continue;
        }
        if (chunk.seq !== lastStdoutSeq + 1) {
          throw new LspProtocolError(`missing stdout seq ${lastStdoutSeq + 1} (received seq ${chunk.seq})`);
        }
        lastStdoutSeq = chunk.seq;
        for (const body of framer.push(data)) {
          connection.handleBody(body);
        }
      }
      if (this.state === 'starting' || this.state === 'running') {
        this.state = 'stopped';
        connection.close(new LspNotRunningError('language server output stream ended'));
        this.stopWatching();
      }
    } catch (err) {
      connection.close(err instanceof Error ? err : new LspProtocolError(String(err)));
      subscription.close();
      this.stopWatching();
      if (this.state === 'running') {
        this.state = 'stopped';
        await command.kill('kill').catch(() => undefined);
      }
    }
  }

  private async watchExit(command: Command): Promise<void> {
    const intervalMs = this.opts.exitPollIntervalMs ?? DEFAULT_EXIT_POLL_INTERVAL_MS;
    let failures = 0;
    while (this.isActive()) {
      await new Promise<void>((resolve) => {
        this.watchWake = resolve;
        this.watchTimer = setTimeout(resolve, intervalMs);
      });
      if (!this.isActive()) {
        return;
      }
      let response;
      try {
        response = await this.client.pollCommand(this.leaseId, command.cmdId);
      } catch {
        failures += 1;
        if (failures >= EXIT_POLL_MAX_CONSECUTIVE_FAILURES && this.isActive()) {
          this.state = 'stopped';
          this.connection?.close(
            new LspNotRunningError(`lost contact with language server: exit polling failed ${failures} times in a row`),
          );
          this.subscription?.close();
          return;
        }
        continue;
      }
      failures = 0;
      if (response.outcome === 'running') {
        continue;
      }
      if (this.isActive()) {
        this.state = 'stopped';
        this.connection?.close(
          new LspServerExitedError(response.outcome === 'completed' ? response.exit_code : undefined),
        );
        this.subscription?.close();
      }
      return;
    }
  }

  private isActive(): boolean {
    return this.state === 'starting' || this.state === 'running';
  }

  private stopWatching(): void {
    clearTimeout(this.watchTimer);
    this.watchWake?.();
  }

  async stop(): Promise<void> {
    this.stopPromise ??= this.doStop();
    return this.stopPromise;
  }

  private async doStop(): Promise<void> {
    if (this.startPromise) {
      await this.startPromise.catch(() => undefined);
    }
    const connection = this.connection;
    const command = this.command;
    if (this.state === 'running' && connection && command) {
      this.state = 'stopping';
      const stopTimeoutMs = this.opts.stopTimeoutMs ?? DEFAULT_STOP_TIMEOUT_MS;
      await connection.request('shutdown', undefined, { timeoutMs: stopTimeoutMs }).catch(() => undefined);
      await connection.notify('exit').catch(() => undefined);
      await command.closeStdin().catch(() => undefined);
      if (!(await this.exited(command, stopTimeoutMs))) {
        await command.kill('term').catch(() => undefined);
        if (!(await this.exited(command, this.opts.killGraceMs ?? DEFAULT_KILL_GRACE_MS))) {
          await command.kill('kill').catch(() => undefined);
        }
      }
    }
    await this.teardown();
  }

  private async exited(command: Command, timeoutMs: number): Promise<boolean> {
    try {
      await command.wait({ timeoutMs, pollIntervalMs: STOP_POLL_INTERVAL_MS });
      return true;
    } catch {
      return false;
    }
  }

  private async teardown(): Promise<void> {
    this.state = 'stopped';
    this.stopWatching();
    this.connection?.close(new LspNotRunningError('language server stopped'));
    this.subscription?.close();
    this.documents.clear();
  }

  async didOpen(path: string, text: string, languageId?: string): Promise<void> {
    const uri = this.uri(path);
    this.documents.set(uri, 1);
    await this.running().notify('textDocument/didOpen', {
      textDocument: { uri, languageId: languageId ?? this.spec.languageId ?? 'plaintext', version: 1, text },
    });
  }

  async didChange(path: string, text: string): Promise<void> {
    const uri = this.uri(path);
    const version = (this.documents.get(uri) ?? 0) + 1;
    this.documents.set(uri, version);
    await this.running().notify('textDocument/didChange', {
      textDocument: { uri, version },
      contentChanges: [{ text }],
    });
  }

  async didClose(path: string): Promise<void> {
    const uri = this.uri(path);
    this.documents.delete(uri);
    await this.running().notify('textDocument/didClose', { textDocument: { uri } });
  }

  documentSymbols(path: string): Promise<DocumentSymbol[] | SymbolInformation[] | null> {
    return this.request('textDocument/documentSymbol', { textDocument: { uri: this.uri(path) } });
  }

  async completions(path: string, position: Position): Promise<CompletionItem[]> {
    const result = await this.request<CompletionList | CompletionItem[] | null>('textDocument/completion', {
      textDocument: { uri: this.uri(path) },
      position,
    });
    if (result === null) {
      return [];
    }
    return Array.isArray(result) ? result : result.items;
  }

  hover(path: string, position: Position): Promise<Hover | null> {
    return this.request('textDocument/hover', { textDocument: { uri: this.uri(path) }, position });
  }

  definition(path: string, position: Position): Promise<Location | Location[] | LocationLink[] | null> {
    return this.request('textDocument/definition', { textDocument: { uri: this.uri(path) }, position });
  }

  request<T = unknown>(method: string, params?: unknown, opts?: { timeoutMs?: number }): Promise<T> {
    try {
      return this.running().request<T>(method, params, opts);
    } catch (err) {
      return Promise.reject(err);
    }
  }

  notify(method: string, params?: unknown): Promise<void> {
    try {
      return this.running().notify(method, params);
    } catch (err) {
      return Promise.reject(err);
    }
  }

  onNotification(method: string, handler: NotificationHandler): () => void {
    this.notificationHandlers.push([method, handler]);
    const detach = this.connection?.onNotification(method, handler);
    return () => {
      const index = this.notificationHandlers.findIndex(([m, h]) => m === method && h === handler);
      if (index >= 0) {
        this.notificationHandlers.splice(index, 1);
      }
      detach?.();
    };
  }

  onStderr(handler: (text: string) => void): () => void {
    this.stderrHandlers.add(handler);
    return () => {
      this.stderrHandlers.delete(handler);
    };
  }

  private running(): JsonRpcConnection {
    if ((this.state !== 'running' && this.state !== 'starting') || !this.connection) {
      throw new LspNotRunningError(`language server is not running (state: ${this.state})`);
    }
    return this.connection;
  }
}

function mergeCapabilities(base: Record<string, unknown>, extra?: Record<string, unknown>): Record<string, unknown> {
  if (!extra) {
    return base;
  }
  const out: Record<string, unknown> = { ...base };
  for (const [key, value] of Object.entries(extra)) {
    const existing = out[key];
    out[key] = isRecord(existing) && isRecord(value) ? mergeCapabilities(existing, value) : value;
  }
  return out;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

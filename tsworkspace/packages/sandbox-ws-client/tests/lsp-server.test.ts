import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { WebSocket as NodeWebSocket } from 'ws';

import type { SandboxWsClient } from '../src/client.ts';
import { leaseId } from '../src/ids.ts';
import {
  encodeMessage,
  LspFramer,
  LspNotRunningError,
  LspProtocolError,
  LspServerExitedError,
} from '../src/lsp/index.ts';
import { Sandbox } from '../src/sandbox.ts';
import {
  connectTestClient,
  onRequest,
  respond,
  respondError,
  send,
  startFakeServer,
  stopFakeServer,
  type RequestFrame,
  type ServerContext,
} from './harness.ts';

interface StdinPayload {
  seq: number;
  data: number[];
  eof: boolean;
}

interface FakeLspOptions {
  exitOnEof?: boolean;
  exitOnSignal?: 'term' | 'kill';
  splitReplies?: boolean;
  duplicateReplies?: boolean;
  exitCode?: number;
  silentInitialize?: boolean;
  pollFailures?: number;
}

class FakeLspGateway {
  readonly stdinFrames: StdinPayload[] = [];
  readonly received: Array<{ id?: number; method?: string; params?: any; result?: unknown }> = [];
  readonly tools: string[] = [];
  runPayload: Record<string, unknown> | undefined;
  subscribeCount = 0;
  pollCalls = 0;
  signals: string[] = [];
  exited = false;
  private socket: NodeWebSocket | undefined;
  private readonly framer = new LspFramer();
  private stdoutSeq = 0;
  private started = false;
  private subscribedBeforeRun = false;

  constructor(
    ctx: ServerContext,
    private readonly opts: FakeLspOptions = {},
  ) {
    onRequest(ctx, (frame, socket) => this.handle(frame, socket));
  }

  get wasSubscribedBeforeRun(): boolean {
    return this.subscribedBeforeRun;
  }

  private handle(frame: RequestFrame, socket: NodeWebSocket): void {
    this.socket = socket;
    this.tools.push(frame.tool);
    const payload = frame.payload as Record<string, unknown>;
    switch (frame.tool) {
      case 'sandbox_subscribe_evidence':
        this.subscribeCount += 1;
        respond(socket, frame, { subscribed: true });
        return;
      case 'sandbox_run_command':
        this.runPayload = payload;
        this.subscribedBeforeRun = this.subscribeCount > 0;
        this.started = true;
        respond(socket, frame, { outcome: 'started', execution_id: 'exec-lsp' });
        return;
      case 'sandbox_write_stdin': {
        const p = payload as unknown as StdinPayload;
        this.stdinFrames.push(p);
        respond(socket, frame, { outcome: 'written', next_seq: p.seq + 1 });
        for (const body of this.framer.push(Uint8Array.from(p.data))) {
          this.onMessage(JSON.parse(body));
        }
        if (p.eof && this.opts.exitOnEof !== false) {
          this.exited = true;
        }
        return;
      }
      case 'sandbox_poll_command':
        this.pollCalls += 1;
        if (this.pollCalls <= (this.opts.pollFailures ?? 0)) {
          respondError(socket, frame, 'transport', 'poll failed');
          return;
        }
        respond(
          socket,
          frame,
          this.exited
            ? { outcome: 'completed', exit_code: this.opts.exitCode ?? 0, stdout: [], stderr: [] }
            : { outcome: 'running' },
        );
        return;
      case 'sandbox_kill_command': {
        const signal = String(payload.signal);
        this.signals.push(signal);
        if (this.opts.exitOnSignal === signal) {
          this.exited = true;
        }
        respond(socket, frame, { outcome: 'signaled' });
        return;
      }
    }
  }

  private onMessage(message: { id?: number; method?: string; params?: any; result?: unknown }): void {
    this.received.push(message);
    switch (message.method) {
      case 'initialize':
        if (!this.opts.silentInitialize) this.reply(message.id!, { capabilities: { hoverProvider: true } });
        break;
      case 'textDocument/documentSymbol':
        this.reply(message.id!, [{ name: 'main', kind: 12 }]);
        break;
      case 'textDocument/completion':
        this.reply(message.id!, { isIncomplete: false, items: [{ label: 'log' }] });
        break;
      case 'shutdown':
        this.reply(message.id!, null);
        break;
      case 'exit':
        this.exited = this.opts.exitOnEof !== false;
        break;
    }
  }

  reply(id: number, result: unknown): void {
    this.emitMessage({ jsonrpc: '2.0', id, result });
  }

  emitMessage(message: unknown): void {
    const bytes = encodeMessage(message);
    if (this.opts.splitReplies) {
      const cut = Math.floor(bytes.length / 2);
      this.emitChunk('stdout', bytes.subarray(0, cut));
      this.emitChunk('stdout', bytes.subarray(cut));
    } else {
      this.emitChunk('stdout', bytes);
    }
  }

  emitChunk(stream: 'stdout' | 'stderr', data: Uint8Array, seq?: number): void {
    const chunkSeq = seq ?? (stream === 'stdout' ? this.stdoutSeq++ : 0);
    const chunk = { execution_id: 'exec-lsp', stream, seq: chunkSeq, data: Array.from(data) };
    send(this.socket!, {
      kind: 'event',
      stream_id: 'lease-lsp',
      event: { subject_suffix: 'command.exec-lsp', payload: JSON.stringify(chunk) },
    });
    if (this.opts.duplicateReplies && stream === 'stdout' && seq === undefined) {
      send(this.socket!, {
        kind: 'event',
        stream_id: 'lease-lsp',
        event: { subject_suffix: 'command.exec-lsp', payload: JSON.stringify(chunk) },
      });
    }
  }

  stdinText(): string {
    return new TextDecoder().decode(Uint8Array.from(this.stdinFrames.flatMap((f) => f.data)));
  }
}

describe('LspServer', () => {
  let ctx: ServerContext;
  let client: SandboxWsClient;
  let sandbox: Sandbox;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    client?.close();
    await stopFakeServer(ctx);
  });

  async function setup(opts?: FakeLspOptions) {
    const gateway = new FakeLspGateway(ctx, opts);
    client = await connectTestClient(ctx);
    sandbox = new Sandbox(client, { sandboxId: leaseId('lease-lsp'), provider: 'local_docker', deadline: new Date() });
    return gateway;
  }

  const fastStop = { stopTimeoutMs: 200, killGraceMs: 100, requestTimeoutMs: 2000 };

  it('runs the full lifecycle and sends framed JSON-RPC over sandbox_write_stdin', async () => {
    const gateway = await setup({ splitReplies: true });
    const server = sandbox.createLspServer('typescript', '/work/app', fastStop);

    const init = await server.start();
    expect(init.capabilities).toEqual({ hoverProvider: true });
    expect(gateway.wasSubscribedBeforeRun).toBe(true);
    expect(gateway.runPayload).toMatchObject({
      lease_id: 'lease-lsp',
      command: ['typescript-language-server', '--stdio'],
      cwd: '/work/app',
      detached: true,
      stream_output: true,
      stdin: 'interactive',
      output_retention: 'stream_only',
      deadline: 'lease',
    });

    await server.didOpen('src/a.ts', 'const é = 1;');
    await server.didChange('src/a.ts', 'const é = 2;');
    expect(await server.documentSymbols('src/a.ts')).toEqual([{ name: 'main', kind: 12 }]);
    expect(await server.completions('src/a.ts', { line: 0, character: 1 })).toEqual([{ label: 'log' }]);
    await server.didClose('src/a.ts');
    await server.stop();

    expect(gateway.stdinFrames.map((f) => f.seq)).toEqual(gateway.stdinFrames.map((_, i) => i));
    expect(gateway.subscribeCount).toBe(1);
    const methods = gateway.received.map((m) => m.method);
    expect(methods).toEqual([
      'initialize',
      'initialized',
      'textDocument/didOpen',
      'textDocument/didChange',
      'textDocument/documentSymbol',
      'textDocument/completion',
      'textDocument/didClose',
      'shutdown',
      'exit',
    ]);
    const [initialize, , didOpen, didChange, symbols] = gateway.received;
    expect(initialize.id).toBe(1);
    expect(initialize.params.rootUri).toBe('file:///work/app');
    expect(didOpen.params.textDocument).toEqual({
      uri: 'file:///work/app/src/a.ts',
      languageId: 'typescript',
      version: 1,
      text: 'const é = 1;',
    });
    expect(didChange.params).toEqual({
      textDocument: { uri: 'file:///work/app/src/a.ts', version: 2 },
      contentChanges: [{ text: 'const é = 2;' }],
    });
    expect(symbols.params.textDocument.uri).toBe('file:///work/app/src/a.ts');
    expect(gateway.stdinFrames.at(-1)).toMatchObject({ seq: gateway.stdinFrames.length - 1, data: [], eof: true });
    expect(gateway.signals).toEqual([]);
  });

  it('accepts a custom spec and resolves absolute paths and escapes URIs', async () => {
    await setup();
    const server = sandbox.createLspServer({ command: 'my-ls', args: ['--x'] }, '/work/app');
    expect(server.uri('/other/a b.ts')).toBe('file:///other/a%20b.ts');
    expect(server.uri('../x.ts')).toBe('file:///work/x.ts');
    expect(() => sandbox.createLspServer('python', 'relative/dir')).toThrow(/absolute/);
  });

  it('drops duplicate stdout chunks by seq', async () => {
    await setup({ duplicateReplies: true });
    const server = sandbox.createLspServer('python', '/p', fastStop);
    await server.start();
    expect(await server.documentSymbols('a.py')).toEqual([{ name: 'main', kind: 12 }]);
    await server.stop();
  });

  it('fails pending requests with LspProtocolError on a stdout gap', async () => {
    const gateway = await setup();
    const server = sandbox.createLspServer('python', '/p', fastStop);
    await server.start();
    const pending = server.request('never/answered');
    await new Promise((r) => setTimeout(r, 20));
    gateway.emitChunk('stdout', new Uint8Array([1]), 99);
    await expect(pending).rejects.toBeInstanceOf(LspProtocolError);
    await expect(pending).rejects.toThrow('missing stdout seq 1 (received seq 99)');
    await new Promise((r) => setTimeout(r, 50));
    expect((client as unknown as { evidenceListeners: Map<string, unknown> }).evidenceListeners.has('lease-lsp')).toBe(
      false,
    );
    expect(gateway.signals).toContain('kill');
    await expect(server.request('after')).rejects.toBeInstanceOf(LspNotRunningError);
  });

  it('answers server requests and surfaces notifications and stderr', async () => {
    const gateway = await setup();
    const server = sandbox.createLspServer('typescript', '/p', fastStop);
    const notes: unknown[] = [];
    const errs: string[] = [];
    server.onNotification('textDocument/publishDiagnostics', (p) => notes.push(p));
    server.onStderr((t) => errs.push(t));
    await server.start();
    gateway.emitMessage({ jsonrpc: '2.0', id: 'srv-1', method: 'workspace/configuration', params: { items: [{}] } });
    gateway.emitMessage({
      jsonrpc: '2.0',
      method: 'textDocument/publishDiagnostics',
      params: { uri: 'file:///p/a.ts', diagnostics: [] },
    });
    gateway.emitChunk('stderr', new TextEncoder().encode('warn'));
    await new Promise((r) => setTimeout(r, 50));
    expect(gateway.received.find((m) => m.id === ('srv-1' as unknown as number))?.result).toEqual([null]);
    expect(notes).toEqual([{ uri: 'file:///p/a.ts', diagnostics: [] }]);
    expect(errs).toEqual(['warn']);
    await server.stop();
  });

  it('stop is idempotent and escalates term then kill when the server ignores eof', async () => {
    const gateway = await setup({ exitOnEof: false, exitOnSignal: 'kill' });
    const server = sandbox.createLspServer('typescript', '/p', { ...fastStop, stopTimeoutMs: 100 });
    await server.start();
    await Promise.all([server.stop(), server.stop()]);
    await server.stop();
    expect(gateway.signals).toEqual(['term', 'kill']);
    expect(gateway.received.filter((m) => m.method === 'shutdown')).toHaveLength(1);
    await expect(server.start()).rejects.toBeInstanceOf(LspNotRunningError);
  });

  it('fails pending requests immediately when the process exits', async () => {
    const gateway = await setup({ exitCode: 3 });
    const server = sandbox.createLspServer('python', '/p', {
      ...fastStop,
      exitPollIntervalMs: 10,
      requestTimeoutMs: 60_000,
    });
    await server.start();
    const pending = server.request('never/answered');
    gateway.exited = true;
    await expect(pending).rejects.toMatchObject({ name: 'LspServerExitedError', exitCode: 3 });
    await expect(pending).rejects.toBeInstanceOf(LspServerExitedError);
    await expect(server.request('after')).rejects.toBeInstanceOf(LspNotRunningError);
    await server.stop();
    await server.stop();
    expect(gateway.received.filter((m) => m.method === 'shutdown')).toHaveLength(0);
  });

  it('keeps watching through transient poll errors', async () => {
    const gateway = await setup({ exitCode: 4, pollFailures: 3 });
    const server = sandbox.createLspServer('python', '/p', {
      ...fastStop,
      exitPollIntervalMs: 5,
      requestTimeoutMs: 60_000,
    });
    await server.start();
    const pending = server.request('never/answered');
    gateway.exited = true;
    await expect(pending).rejects.toMatchObject({ name: 'LspServerExitedError', exitCode: 4 });
  });

  it('fails pending requests after repeated consecutive poll failures', async () => {
    await setup({ pollFailures: 1000 });
    const server = sandbox.createLspServer('python', '/p', {
      ...fastStop,
      exitPollIntervalMs: 5,
      requestTimeoutMs: 60_000,
    });
    await server.start();
    await expect(server.request('never/answered')).rejects.toThrow(/exit polling failed 5 times/);
    await expect(server.request('after')).rejects.toBeInstanceOf(LspNotRunningError);
  });

  it('fails initialize when the process exits during start', async () => {
    const gateway = await setup({ exitCode: 1, silentInitialize: true });
    gateway.exited = true;
    const server = sandbox.createLspServer('python', '/p', {
      ...fastStop,
      exitPollIntervalMs: 10,
      requestTimeoutMs: 60_000,
    });
    const started = server.start();
    await expect(started).rejects.toBeInstanceOf(LspServerExitedError);
  });

  it('stop before start is a no-op and requests before start fail', async () => {
    const gateway = await setup();
    const server = sandbox.createLspServer('typescript', '/p', fastStop);
    await expect(server.request('x')).rejects.toBeInstanceOf(LspNotRunningError);
    await server.stop();
    expect(gateway.tools).toEqual([]);
  });
});

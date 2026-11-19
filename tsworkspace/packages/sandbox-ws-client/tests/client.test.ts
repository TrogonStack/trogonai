import type { AddressInfo } from 'node:net';

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WebSocket as NodeWebSocket, WebSocketServer } from 'ws';

import { createSession, SandboxWsClient } from '../src/client.ts';
import { LeaseDeniedError } from '../src/errors.ts';
import { leaseId, tenantId } from '../src/ids.ts';
import type { WsFrame } from '../src/protocol.ts';
import type { LeaseRequest } from '../src/wire.ts';

interface ServerContext {
  wss: WebSocketServer;
  url: string;
}

function startFakeServer(): Promise<ServerContext> {
  return new Promise((resolve) => {
    const wss = new WebSocketServer({ port: 0, host: '127.0.0.1' });
    wss.on('listening', () => {
      const { port } = wss.address() as AddressInfo;
      resolve({ wss, url: `ws://127.0.0.1:${port}` });
    });
  });
}

function stopFakeServer(ctx: ServerContext): Promise<void> {
  return new Promise((resolve) => {
    ctx.wss.close(() => resolve());
  });
}

function send(socket: NodeWebSocket, frame: WsFrame): void {
  socket.send(JSON.stringify(frame));
}

// Judgment call: SandboxWsClient.connect() performs a real HTTP POST to
// gatewayHttpUrl before opening the WebSocket. These tests exercise the
// tool-dispatch logic against a WS-only fake server, so they open the WS leg
// directly through SandboxWsClient's private constructor rather than going
// through connect(). The HTTP session step is covered separately below by
// mocking global fetch, and connect()'s handshake-failure path is exercised
// indirectly through the same private constructor's real WsFrame demuxing.
async function connectTestClient(ctx: ServerContext): Promise<SandboxWsClient> {
  const ClientCtor = SandboxWsClient as unknown as {
    new (socket: WebSocket): SandboxWsClient;
  };
  const socket = new NodeWebSocket(`${ctx.url}/v1/ws`, ['bearer.test-token']) as unknown as WebSocket;
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve());
    socket.addEventListener('error', (event: Event) => reject(event));
  });
  return new ClientCtor(socket);
}

describe('SandboxWsClient tool dispatch over a fake gateway', () => {
  let ctx: ServerContext;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  it('sends a correctly shaped sandbox_lease request frame and resolves on success', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_lease') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              lease_id: 'lease-abc',
              provider: 'mock',
              sandbox_id: 'sandbox-1',
              evidence_subject: 'evidence.subject',
              deadline: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
            },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const request: LeaseRequest = {
      tenant: tenantId('tenant-a'),
      provider_pref: ['mock'],
      spec: {
        image: { kind: 'registry', reference: { registry: 'r', repository: 'repo' } },
        workspace: { empty: {} },
        env: {},
        cpu_seconds: 60,
        memory_mib: 512,
        mounts: [],
      },
      ttl_seconds: 120,
      idempotency_key: 'idem-1',
      host_imports: [],
      prewarm: false,
    };

    const granted = await client.lease(request, { timeoutSeconds: 45 });
    expect(granted.lease_id).toBe('lease-abc');
    expect(granted.provider).toBe('mock');

    expect(receivedFrame?.kind).toBe('request');
    expect(receivedFrame?.tool).toBe('sandbox_lease');
    expect(typeof (receivedFrame as { id: string }).id).toBe('string');
    const sentPayload = (receivedFrame as { payload: Record<string, unknown> }).payload;
    expect(sentPayload.tenant).toBe('tenant-a');
    expect(sentPayload.idempotency_key).toBe('idem-1');
    expect(sentPayload.timeout_seconds).toBe(45);

    client.close();
  });

  it('sends tags on sandbox_list_leases only when a filter is given', async () => {
    const payloads: Record<string, unknown>[] = [];
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_list_leases') {
          payloads.push(frame.payload as Record<string, unknown>);
          send(socket, { kind: 'response', id: frame.id, result: { leases: [] } });
        }
      });
    });

    const client = await connectTestClient(ctx);
    await client.listLeases();
    await client.listLeases({ tags: {} });
    await client.listLeases({ tags: { app: 'web', env: 'dev' } });

    expect(payloads).toEqual([{}, {}, { tags: { app: 'web', env: 'dev' } }]);
    client.close();
  });

  it('sends the reap filters on sandbox_reap_leases and returns the typed response', async () => {
    let sent: Record<string, unknown> | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_reap_leases') {
          sent = frame.payload as Record<string, unknown>;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'reaped', dry_run: true, matched: [], released: [], failed: [] },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const response = await client.reapLeases({
      tags: { app: 'web' },
      older_than_seconds: 3600,
      dry_run: true,
    });

    expect(sent).toEqual({ tags: { app: 'web' }, older_than_seconds: 3600, dry_run: true });
    expect(response.outcome).toBe('reaped');
    client.close();
  });

  it('surfaces lease_denied errors as a rejected LeaseDeniedError', async () => {
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_lease') {
          const payload = frame.payload as { idempotency_key: string };
          if (payload.idempotency_key.includes('deny')) {
            send(socket, {
              kind: 'response',
              id: frame.id,
              error: { code: 'lease_denied', message: 'Debug("no capacity")' },
            });
          }
        }
      });
    });

    const client = await connectTestClient(ctx);
    const request: LeaseRequest = {
      tenant: tenantId('tenant-a'),
      provider_pref: ['mock'],
      spec: {
        image: { kind: 'registry', reference: { registry: 'r', repository: 'repo' } },
        workspace: { empty: {} },
        env: {},
        cpu_seconds: 60,
        memory_mib: 512,
        mounts: [],
      },
      ttl_seconds: 120,
      idempotency_key: 'please-deny-me',
      host_imports: [],
      prewarm: false,
    };

    await expect(client.lease(request)).rejects.toBeInstanceOf(LeaseDeniedError);
    client.close();
  });

  it('correlates responses by id even when the server answers out of order', async () => {
    ctx.wss.on('connection', (socket) => {
      const seen: WsFrame[] = [];
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_cancel') {
          seen.push(frame);
          if (seen.length === 2) {
            // Respond to the second request first, then the first, to prove
            // id-based correlation rather than FIFO ordering.
            send(socket, { kind: 'response', id: seen[1].id, result: { cancelled: true } });
            send(socket, { kind: 'response', id: seen[0].id, result: { cancelled: true } });
          }
        }
      });
    });

    const client = await connectTestClient(ctx);
    const first = client.cancel(leaseId('lease-first'));
    const second = client.cancel(leaseId('lease-second'));

    await expect(Promise.all([first, second])).resolves.toEqual([undefined, undefined]);
    client.close();
  });

  it('demuxes evidence events by stream_id and decodes plain text and base64 payloads', async () => {
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_subscribe_evidence') {
          const payload = frame.payload as { lease_id: string };
          send(socket, { kind: 'response', id: frame.id, result: { subscribed: true } });
          send(socket, {
            kind: 'event',
            stream_id: payload.lease_id,
            event: { subject_suffix: 'stdout', payload: 'hello world' },
          });
          const encoded = Buffer.from([1, 2, 3, 4]).toString('base64');
          send(socket, {
            kind: 'event',
            stream_id: payload.lease_id,
            event: { subject_suffix: 'artifact', payload: `base64:${encoded}` },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const stream = client.evidenceStream(leaseId('lease-evidence'));
    const iterator = stream[Symbol.asyncIterator]();

    const first = await iterator.next();
    expect(first.done).toBe(false);
    expect(first.value).toEqual({ subjectSuffix: 'stdout', payload: 'hello world' });

    const second = await iterator.next();
    expect(second.done).toBe(false);
    expect(second.value.subjectSuffix).toBe('artifact');
    expect(second.value.payload).toBeInstanceOf(Uint8Array);
    expect(Array.from(second.value.payload as Uint8Array)).toEqual([1, 2, 3, 4]);

    client.close();
  });

  it('sends a correctly shaped sandbox_run_command request frame and resolves on success', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_run_command') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'completed', exit_code: 0, stdout: [104, 105], stderr: [] },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const result = await client.runCommand(leaseId('lease-run'), ['echo', 'hi'], {
      cwd: '/workspace',
      timeoutSeconds: 30,
      runAsUser: 'alice',
    });
    expect(result).toEqual({ outcome: 'completed', exit_code: 0, stdout: [104, 105], stderr: [] });

    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_run_command',
      payload: {
        lease_id: 'lease-run',
        command: ['echo', 'hi'],
        cwd: '/workspace',
        env: undefined,
        timeout_seconds: 30,
        detached: undefined,
        run_as_user: 'alice',
        launch_key: expect.stringMatching(/^[A-Za-z0-9_-]{1,128}$/),
      },
    });

    client.close();
  });

  it('sends a correctly shaped sandbox_create_user request frame and resolves to a SandboxUser', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_create_user') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'created', home_dir: '/home/alice' },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const user = await client.createUser(leaseId('lease-user'), 'alice');
    expect(user.username).toBe('alice');
    expect(user.homeDir).toBe('/home/alice');

    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_create_user',
      payload: { lease_id: 'lease-user', username: 'alice' },
    });

    client.close();
  });

  it('SandboxUser.runCommand injects run_as_user and defaults cwd to the home directory', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_run_command') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'completed', exit_code: 0, stdout: [], stderr: [] },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const user = client.asUser(leaseId('lease-user'), 'bob');
    await user.runCommand(['ls']);

    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_run_command',
      payload: {
        lease_id: 'lease-user',
        command: ['ls'],
        cwd: '/home/bob',
        env: undefined,
        timeout_seconds: undefined,
        detached: undefined,
        run_as_user: 'bob',
        launch_key: expect.stringMatching(/^[A-Za-z0-9_-]{1,128}$/),
      },
    });

    client.close();
  });

  it.each(['Alice', 'user name', '$(whoami)'])('rejects invalid sandbox usernames client-side: %s', async (name) => {
    const client = await connectTestClient(ctx);
    await expect(client.createUser(leaseId('lease-invalid'), name)).rejects.toThrow();
    expect(() => client.asUser(leaseId('lease-invalid'), name)).toThrow();
    client.close();
  });

  it('rejects with unknown_tool for an unrecognized tool name', async () => {
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'not_a_real_tool') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            error: { code: 'unknown_tool', message: 'unknown tool: not_a_real_tool' },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const internalDispatch = client as unknown as {
      dispatch: <T>(tool: string, payload: unknown) => Promise<T>;
    };
    await expect(internalDispatch.dispatch('not_a_real_tool', {})).rejects.toMatchObject({
      code: 'unknown_tool',
    });
    client.close();
  });
});

describe('createSession HTTP flow', () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('sends the X-Sandbox-API-Key header and parses the token response', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      expect(init?.method).toBe('POST');
      expect(init?.headers).toMatchObject({ 'X-Sandbox-API-Key': 'secret-key' });
      return new Response(JSON.stringify({ token: 'tok-123', expires_at: 1234567890 }), {
        status: 201,
      });
    });
    globalThis.fetch = fetchMock as unknown as typeof fetch;

    const result = await createSession('https://gateway.example.com', 'secret-key');
    expect(result).toEqual({ token: 'tok-123', expires_at: 1234567890 });
    expect(fetchMock).toHaveBeenCalledWith(
      'https://gateway.example.com/v1/sessions',
      expect.objectContaining({ method: 'POST' }),
    );
  });

  it('surfaces a non-201 response as a thrown error', async () => {
    globalThis.fetch = vi.fn(async () => {
      return new Response(JSON.stringify({ error: 'invalid api key' }), { status: 401 });
    }) as unknown as typeof fetch;

    await expect(createSession('https://gateway.example.com', 'bad-key')).rejects.toThrow('invalid api key');
  });
});

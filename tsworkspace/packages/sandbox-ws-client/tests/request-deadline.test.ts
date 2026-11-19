import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { WebSocket as NodeWebSocket } from 'ws';

import { DEFAULT_REQUEST_TIMEOUT_MS, SandboxWsClient } from '../src/client.ts';
import { TimeoutError } from '../src/errors.ts';
import { FILE_TRANSFER_CHUNK_BYTES } from '../src/fileops.ts';
import { leaseId } from '../src/ids.ts';
import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { FileOp } from '../src/wire.ts';
import { onRequest, respond, startFakeServer, stopFakeServer, type ServerContext } from './harness.ts';

const LEASE = leaseId('lease-deadline');

async function connectWithDeadline(ctx: ServerContext, requestTimeoutMs?: number): Promise<SandboxWsClient> {
  const ClientCtor = SandboxWsClient as unknown as {
    new (socket: WebSocket, retry?: false, requestTimeoutMs?: number): SandboxWsClient;
  };
  const socket = new NodeWebSocket(`${ctx.url}/v1/ws`, ['bearer.test-token']) as unknown as WebSocket;
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve());
    socket.addEventListener('error', (event: Event) => reject(event));
  });
  return new ClientCtor(socket, false, requestTimeoutMs);
}

describe('client-wide request deadline', () => {
  let ctx: ServerContext;
  let client: SandboxWsClient | undefined;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    await client?.close();
    client = undefined;
    await stopFakeServer(ctx);
  });

  it('rejects with TimeoutError when no reply arrives in time', async () => {
    onRequest(ctx, () => {});
    client = await connectWithDeadline(ctx, 50);
    const err = await client.listLeases().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(TimeoutError);
    expect(err).toMatchObject({ code: 'timeout' });
    expect((err as TimeoutError).message).toContain('sandbox_list_leases');
  });

  it('resolves normally when the reply beats the deadline', async () => {
    onRequest(ctx, (frame, socket) => respond(socket, frame, { leases: [] }));
    client = await connectWithDeadline(ctx, 1_000);
    await expect(client.listLeases()).resolves.toEqual({ leases: [] });
  });

  it('adds a long poll wait on top of the default', async () => {
    onRequest(ctx, (frame, socket) => {
      setTimeout(() => respond(socket, frame, { outcome: 'unsupported' }), 150);
    });
    client = await connectWithDeadline(ctx, 50);
    await expect(client.readLogs(LEASE, 'exec-1', { waitMs: 500 })).resolves.toEqual({ outcome: 'unsupported' });
  });

  it('applies the 10 s default when none is configured', async () => {
    onRequest(ctx, (frame, socket) => {
      setTimeout(() => respond(socket, frame, { leases: [] }), 100);
    });
    client = await connectWithDeadline(ctx);
    expect(DEFAULT_REQUEST_TIMEOUT_MS).toBe(10_000);
    expect((client as unknown as { requestTimeoutMs: number }).requestTimeoutMs).toBe(DEFAULT_REQUEST_TIMEOUT_MS);
    await expect(client.listLeases()).resolves.toEqual({ leases: [] });
  });

  it('adds the gateway fork wait on top of the default', async () => {
    onRequest(ctx, (frame, socket) => {
      setTimeout(() => respond(socket, frame, { cancelled: true }), 150);
    });
    client = await connectWithDeadline(ctx, 50);
    await expect(client.cancel(LEASE)).rejects.toBeInstanceOf(TimeoutError);
    await expect(client.fork(LEASE, 60, 'fork-key')).resolves.toBeDefined();
  });

  it('gives a terminal open time for a cold provider on top of the default', async () => {
    onRequest(ctx, (frame, socket) => {
      setTimeout(() => respond(socket, frame, { outcome: 'opened', terminal_id: 'term-1' }), 150);
    });
    client = await connectWithDeadline(ctx, 50);
    const opened = await client.openTerminal({ lease_id: LEASE } as Parameters<SandboxWsClient['openTerminal']>[0]);
    expect(opened.terminalId).toBe('term-1');
  });

  it('gives an evidence subscription acknowledgement time on top of the default', async () => {
    onRequest(ctx, (frame, socket) => {
      setTimeout(() => respond(socket, frame, { subscribed: true }), 150);
    });
    client = await connectWithDeadline(ctx, 50);
    const subscription = client.subscribeEvidence(LEASE);
    await expect(subscription.ready).resolves.toBeUndefined();
    subscription.close();
  });

  it('refuses a non-positive deadline before connecting', async () => {
    await expect(
      SandboxWsClient.connectWithToken({ gatewayWsUrl: ctx.url, token: 't', requestTimeoutMs: 0 }),
    ).rejects.toBeInstanceOf(RangeError);
  });
});

describe('file transfer abort signal', () => {
  let ctx: ServerContext;
  let client: SandboxWsClient | undefined;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    await client?.close();
    client = undefined;
    await stopFakeServer(ctx);
  });

  it('stops a chunked upload between chunks', async () => {
    const controller = new AbortController();
    const writes: FileOp[] = [];
    onRequest(ctx, (frame, socket) => {
      const op = (frame.payload as { op: FileOp }).op;
      writes.push(op);
      controller.abort(new Error('stop the upload'));
      respond(socket, frame, { outcome: 'written', bytes: 1, size: 1 });
    });
    client = await connectWithDeadline(ctx);
    const data = new Uint8Array(FILE_TRANSFER_CHUNK_BYTES * 3);
    await expect(client.uploadFile(LEASE, '/big.bin', data, { signal: controller.signal })).rejects.toThrow(
      'stop the upload',
    );
    expect(writes).toHaveLength(1);
  });

  it('refuses to start when the signal is already aborted', async () => {
    const memory = new MemorySandboxClient();
    const sandbox = await Sandbox.create(memory, { tenant: tenantId('acme'), timeoutMs: 60_000 });
    const signal = AbortSignal.abort(new Error('cancelled'));
    await expect(sandbox.fs.uploadFile('/a.txt', new Uint8Array([1]), { signal })).rejects.toThrow('cancelled');
    await expect(sandbox.fs.downloadDir('/', { signal })).rejects.toThrow('cancelled');
    await expect(sandbox.fs.uploadDir('/d', [], { signal })).rejects.toThrow('cancelled');
    await expect(sandbox.fs.downloadFile('/a.txt', { signal })).rejects.toThrow('cancelled');
  });
});

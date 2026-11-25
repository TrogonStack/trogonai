import type { AddressInfo } from 'node:net';

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { WebSocket as NodeWebSocket, WebSocketServer } from 'ws';

import { SandboxWsClient } from '../src/client.ts';
import { classify } from '../src/error-classes.ts';
import { SandboxWsError } from '../src/errors.ts';
import { leaseId, tenantId } from '../src/ids.ts';
import { MemorySandboxClient } from '../src/memory.ts';
import type { WsFrame } from '../src/protocol.ts';
import { Sandbox } from '../src/sandbox.ts';

async function connect(wss: WebSocketServer): Promise<SandboxWsClient> {
  const { port } = wss.address() as AddressInfo;
  const ClientCtor = SandboxWsClient as unknown as { new (socket: WebSocket): SandboxWsClient };
  const socket = new NodeWebSocket(`ws://127.0.0.1:${port}/v1/ws`, ['bearer.test-token']) as unknown as WebSocket;
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve());
    socket.addEventListener('error', (event: Event) => reject(event));
  });
  return new ClientCtor(socket);
}

describe('suspend and wake over a fake gateway', () => {
  let wss: WebSocketServer;
  const seen: { tool: string; payload: Record<string, unknown> }[] = [];

  beforeEach(async () => {
    seen.length = 0;
    wss = await new Promise<WebSocketServer>((resolve) => {
      const server = new WebSocketServer({ port: 0, host: '127.0.0.1' });
      server.on('listening', () => resolve(server));
    });
    wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind !== 'request') return;
        seen.push({ tool: frame.tool, payload: frame.payload as Record<string, unknown> });
        if (frame.tool === 'sandbox_wake_lease' && (frame.payload as { lease_id: string }).lease_id === 'busy') {
          socket.send(
            JSON.stringify({
              kind: 'response',
              id: frame.id,
              error: {
                code: 'lease_denied',
                message: 'lease busy is waking',
                kind: 'lease_waking',
                retryable: true,
                retry_after_ms: 1000,
                lease_id: 'busy',
              },
            }),
          );
          return;
        }
        const results: Record<string, unknown> = {
          sandbox_suspend_lease: {
            outcome: 'suspended',
            level: 'memory',
            reap_at: { secs_since_epoch: 100, nanos_since_epoch: 0 },
          },
          sandbox_wake_lease: { outcome: 'woken', deadline: { secs_since_epoch: 200, nanos_since_epoch: 0 } },
        };
        socket.send(JSON.stringify({ kind: 'response', id: frame.id, result: results[frame.tool] }));
      });
    });
  });

  afterEach(async () => {
    await new Promise<void>((resolve) => wss.close(() => resolve()));
  });

  it('sends the verbs with their wire field names and omits unset options', async () => {
    const client = await connect(wss);
    const lease = leaseId('lease-1');
    expect((await client.suspendLease(lease)).outcome).toBe('suspended');
    await client.suspendLease(lease, { level: 'disk', maxSuspendedSeconds: 600 });
    expect((await client.wakeLease(lease)).outcome).toBe('woken');
    expect(seen[0]).toEqual({ tool: 'sandbox_suspend_lease', payload: { lease_id: 'lease-1' } });
    expect(seen[1]?.payload).toEqual({ lease_id: 'lease-1', level: 'disk', max_suspended_seconds: 600 });
    expect(seen[2]).toEqual({ tool: 'sandbox_wake_lease', payload: { lease_id: 'lease-1' } });
    client.close();
  });

  it('surfaces a waking lease as a retryable error with its delay', async () => {
    const client = await connect(wss);
    const err = await client.wakeLease(leaseId('busy')).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SandboxWsError);
    const e = err as SandboxWsError;
    expect(e.variant).toBe('lease_waking');
    expect(e.retryable).toBe(true);
    expect(e.retryAfterMs).toBe(1000);
    client.close();
  });
});

describe('suspend and wake error classes', () => {
  it('classifies the three new kinds', () => {
    expect(classify({ code: 'lease_denied', kind: 'lease_waking' }).retryable).toBe(true);
    expect(classify({ code: 'lease_denied', kind: 'suspend_unsupported' }).retryable).toBe(false);
    expect(classify({ code: 'lease_denied', kind: 'suspend_artifact_lost' }).retryable).toBe(false);
  });
});

describe('MemorySandboxClient suspend and wake', () => {
  async function create() {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
    return { client, sandbox };
  }

  it('suspends and wakes idempotently, reporting the lease status in usage', async () => {
    const { sandbox } = await create();
    const first = await sandbox.suspend({ maxSuspendedMs: 120_000 });
    expect(first.level).toBe('memory');
    expect(await sandbox.suspend()).toEqual(first);
    expect((await sandbox.usage()).status).toBe('suspended');
    const deadline = await sandbox.wake();
    expect(await sandbox.wake()).toEqual(deadline);
    const usage = await sandbox.usage();
    expect(usage.status).toBe('running');
    expect(usage.suspendedSeconds).toBe(0);
  });

  it('refuses a level the fake does not offer as suspend_unsupported', async () => {
    const { sandbox } = await create();
    const err = await sandbox.suspend({ level: 'disk' }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SandboxWsError);
    expect((err as SandboxWsError).variant).toBe('suspend_unsupported');
  });
});

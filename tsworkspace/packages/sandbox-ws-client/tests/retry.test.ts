import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { WebSocket as NodeWebSocket } from 'ws';

import { SandboxWsClient } from '../src/client.ts';
import { SandboxWsError } from '../src/errors.ts';
import { leaseId, tenantId } from '../src/ids.ts';
import type { RetryOptions } from '../src/retry.ts';
import { IDEMPOTENT_TOOLS, resolveRetryPolicy, retryDelayMs, withRetry } from '../src/retry.ts';
import type { LeaseRequest } from '../src/wire.ts';
import type { RequestFrame, ServerContext } from './harness.ts';
import { onRequest, respond, send, startFakeServer, stopFakeServer } from './harness.ts';

const opened: SandboxWsClient[] = [];

async function connect(ctx: ServerContext, retry?: RetryOptions | false): Promise<SandboxWsClient> {
  const ClientCtor = SandboxWsClient as unknown as {
    new (socket: WebSocket, retry?: RetryOptions | false): SandboxWsClient;
  };
  const socket = new NodeWebSocket(`${ctx.url}/v1/ws`, ['bearer.test-token']) as unknown as WebSocket;
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve());
    socket.addEventListener('error', (event: Event) => reject(event));
  });
  const client = new ClientCtor(socket, retry);
  opened.push(client);
  return client;
}

function fail(socket: NodeWebSocket, frame: RequestFrame, code: string, extras: Record<string, unknown> = {}): void {
  send(socket, { kind: 'response', id: frame.id, error: { code, message: code, ...extras } } as never);
}

/** Fast, deterministic options: records every wait instead of sleeping. */
function fastOptions(waits: number[], extra: RetryOptions = {}): RetryOptions {
  return {
    jitter: false,
    sleep: async (ms) => {
      waits.push(ms);
    },
    ...extra,
  };
}

const LEASES = { leases: [] };

describe('SandboxWsClient retry', () => {
  let ctx: ServerContext;
  let seen: string[];

  beforeEach(async () => {
    ctx = await startFakeServer();
    seen = [];
  });

  afterEach(async () => {
    for (const client of opened.splice(0)) {
      client.close();
    }
    await stopFakeServer(ctx);
  });

  it('retries an idempotent read after a transient error and then succeeds', async () => {
    let failures = 2;
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      if (failures > 0) {
        failures -= 1;
        fail(socket, frame, 'backend');
        return;
      }
      respond(socket, frame, LEASES);
    });
    const waits: number[] = [];
    const client = await connect(ctx, fastOptions(waits));
    await expect(client.listLeases()).resolves.toEqual(LEASES);
    expect(seen).toEqual(['sandbox_list_leases', 'sandbox_list_leases', 'sandbox_list_leases']);
    expect(waits).toEqual([100, 200]);
  });

  it('bounds the attempts and raises the last error', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      fail(socket, frame, 'backend');
    });
    const waits: number[] = [];
    const client = await connect(ctx, fastOptions(waits, { maxAttempts: 4 }));
    await expect(client.listLeases()).rejects.toMatchObject({ code: 'backend', retryable: true });
    expect(seen.length).toBe(4);
    expect(waits.length).toBe(3);
  });

  it('never retries a non-idempotent verb, even for a retryable error', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      fail(socket, frame, 'timeout');
    });
    const waits: number[] = [];
    const client = await connect(ctx, fastOptions(waits));
    await expect(
      client.writeStdin(leaseId('l1'), 'e1' as never, 0 as never, new Uint8Array([1]), false),
    ).rejects.toBeDefined();
    expect(seen).toEqual(['sandbox_write_stdin']);
    expect(waits).toEqual([]);
  });

  it('retries a run_command under one launch key so the broker replays it', async () => {
    const keys: string[] = [];
    onRequest(ctx, (frame, socket) => {
      keys.push((frame.payload as { launch_key: string }).launch_key);
      if (keys.length === 1) {
        fail(socket, frame, 'timeout');
        return;
      }
      respond(socket, frame, { outcome: 'completed', exit_code: 0, stdout: [], stderr: [] });
    });
    const client = await connect(ctx, fastOptions([]));
    await client.runCommand(leaseId('l1'), ['echo', 'x']);
    expect(keys).toHaveLength(2);
    expect(keys[0]).toMatch(/^[A-Za-z0-9_-]{1,128}$/);
    expect(keys[1]).toBe(keys[0]);
  });

  it('sends a caller launch key and gives each call its own generated key', async () => {
    const keys: string[] = [];
    onRequest(ctx, (frame, socket) => {
      keys.push((frame.payload as { launch_key: string }).launch_key);
      respond(socket, frame, { outcome: 'completed', exit_code: 0, stdout: [], stderr: [] });
    });
    const client = await connect(ctx, fastOptions([]));
    await client.runCommand(leaseId('l1'), ['true'], { launchKey: 'deploy-7' });
    await client.runCommand(leaseId('l1'), ['true']);
    await client.runCommand(leaseId('l1'), ['true']);
    expect(keys[0]).toBe('deploy-7');
    expect(keys[1]).not.toBe(keys[2]);
    await expect(client.runCommand(leaseId('l1'), ['true'], { launchKey: 'bad key' })).rejects.toMatchObject({
      code: 'invalid_request',
    });
    expect(keys).toHaveLength(3);
  });

  it('does not retry a permanent error', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      fail(socket, frame, 'tenant_mismatch');
    });
    const waits: number[] = [];
    const client = await connect(ctx, fastOptions(waits));
    await expect(client.listLeases()).rejects.toMatchObject({ code: 'tenant_mismatch' });
    expect(seen.length).toBe(1);
  });

  it('honors the gateway retryable flag over the table', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      if (seen.length === 1) {
        fail(socket, frame, 'internal', { retryable: true });
        return;
      }
      respond(socket, frame, LEASES);
    });
    const client = await connect(ctx, fastOptions([]));
    await expect(client.listLeases()).resolves.toEqual(LEASES);
    expect(seen.length).toBe(2);
  });

  it('waits at least as long as retry_after_ms', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      if (seen.length === 1) {
        fail(socket, frame, 'backend', { retry_after_ms: 1500 });
        return;
      }
      respond(socket, frame, LEASES);
    });
    const waits: number[] = [];
    const client = await connect(ctx, fastOptions(waits));
    await client.listLeases();
    expect(waits).toEqual([1500]);
  });

  it('reads retry_after_seconds like an HTTP Retry-After', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      if (seen.length === 1) {
        fail(socket, frame, 'backend', { retry_after_seconds: 2 });
        return;
      }
      respond(socket, frame, LEASES);
    });
    const waits: number[] = [];
    const client = await connect(ctx, fastOptions(waits));
    await client.listLeases();
    expect(waits).toEqual([2000]);
  });

  it('raises at once when Retry-After exceeds maxRetryAfterMs', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      fail(socket, frame, 'backend', { retry_after_ms: 60_000 });
    });
    const waits: number[] = [];
    const client = await connect(ctx, fastOptions(waits, { maxRetryAfterMs: 5000 }));
    await expect(client.listLeases()).rejects.toMatchObject({ retryAfterMs: 60_000 });
    expect(seen.length).toBe(1);
    expect(waits).toEqual([]);
  });

  it('is disabled by retry: false', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      fail(socket, frame, 'backend');
    });
    const client = await connect(ctx, false);
    await expect(client.listLeases()).rejects.toMatchObject({ code: 'backend' });
    expect(seen.length).toBe(1);
  });

  it('is disabled by maxAttempts: 1', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      fail(socket, frame, 'backend');
    });
    const client = await connect(ctx, { maxAttempts: 1 });
    await expect(client.listLeases()).rejects.toMatchObject({ code: 'backend' });
    expect(seen.length).toBe(1);
  });

  it('retries a lease request, which carries an idempotency key, with the same payload', async () => {
    const keys: string[] = [];
    onRequest(ctx, (frame, socket) => {
      keys.push((frame.payload as { idempotency_key: string }).idempotency_key);
      if (keys.length === 1) {
        fail(socket, frame, 'backend');
        return;
      }
      respond(socket, frame, { lease_id: 'l1' });
    });
    const client = await connect(ctx, fastOptions([]));
    const request = {
      tenant: tenantId('acme'),
      idempotency_key: 'key-1',
    } as unknown as LeaseRequest;
    await client.lease(request);
    expect(keys).toEqual(['key-1', 'key-1']);
  });

  it('reports each retry through onRetry', async () => {
    onRequest(ctx, (frame, socket) => {
      seen.push(frame.tool);
      if (seen.length === 1) {
        fail(socket, frame, 'backend');
        return;
      }
      respond(socket, frame, LEASES);
    });
    const events: Array<{ tool: string; attempt: number; delayMs: number }> = [];
    const client = await connect(
      ctx,
      fastOptions([], { onRetry: ({ tool, attempt, delayMs }) => events.push({ tool, attempt, delayMs }) }),
    );
    await client.listLeases();
    expect(events).toEqual([{ tool: 'sandbox_list_leases', attempt: 1, delayMs: 100 }]);
  });
});

describe('retry policy', () => {
  it('only read-only tools and keyed or convergent mutations are idempotent', () => {
    for (const tool of [
      'sandbox_run_command',
      'sandbox_write_stdin',
      'sandbox_write_files',
      'sandbox_file_op',
      'sandbox_kill_command',
      'sandbox_delete_lease',
      'sandbox_fork',
      'sandbox_subscribe_evidence',
    ]) {
      expect(IDEMPOTENT_TOOLS.has(tool)).toBe(false);
    }
    for (const tool of [
      'sandbox_list_leases',
      'sandbox_read_file',
      'sandbox_poll_command',
      'sandbox_lease',
      'sandbox_update_tags',
    ]) {
      expect(IDEMPOTENT_TOOLS.has(tool)).toBe(true);
    }
  });

  it('backs off exponentially up to the cap', () => {
    const policy = resolveRetryPolicy({ jitter: false, baseDelayMs: 100, maxDelayMs: 500 });
    expect([1, 2, 3, 4, 5].map((n) => retryDelayMs(policy, n))).toEqual([100, 200, 400, 500, 500]);
  });

  it('applies full jitter within the ceiling', () => {
    const low = resolveRetryPolicy({ random: () => 0, baseDelayMs: 100 });
    const high = resolveRetryPolicy({ random: () => 0.999999, baseDelayMs: 100 });
    expect(retryDelayMs(low, 1)).toBe(0);
    expect(retryDelayMs(high, 1)).toBe(100);
    expect(retryDelayMs(high, 3)).toBe(400);
  });

  it('never goes below Retry-After, even with jitter', () => {
    const policy = resolveRetryPolicy({ random: () => 0 });
    expect(retryDelayMs(policy, 1, 750)).toBe(750);
  });

  it('rejects invalid options', () => {
    expect(() => resolveRetryPolicy({ maxAttempts: 0 })).toThrow(RangeError);
    expect(() => resolveRetryPolicy({ maxAttempts: 1.5 })).toThrow(RangeError);
    expect(() => resolveRetryPolicy({ baseDelayMs: -1 })).toThrow(RangeError);
  });

  it('does not retry non-library errors or when the connection is closed', async () => {
    const policy = resolveRetryPolicy(fastOptions([]));
    let calls = 0;
    await expect(
      withRetry(
        'sandbox_list_leases',
        policy,
        () => true,
        async () => {
          calls += 1;
          throw new Error('boom');
        },
      ),
    ).rejects.toThrow('boom');
    expect(calls).toBe(1);
    await expect(
      withRetry(
        'sandbox_list_leases',
        policy,
        () => false,
        async () => {
          calls += 1;
          throw new SandboxWsError('backend', 'x');
        },
      ),
    ).rejects.toBeInstanceOf(SandboxWsError);
    expect(calls).toBe(2);
  });
});

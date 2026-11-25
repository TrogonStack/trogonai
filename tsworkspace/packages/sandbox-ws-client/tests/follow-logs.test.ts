import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import type { SandboxClientApi } from '../src/client-api.ts';
import { leaseId, tenantId } from '../src/ids.ts';
import { MemorySandboxClient } from '../src/memory.ts';
import { Command, Sandbox } from '../src/sandbox.ts';
import { connectTestClient, onRequest, respond, send, startFakeServer, stopFakeServer } from './harness.ts';
import type { ServerContext } from './harness.ts';

const CHUNKS = [
  { log_seq: 3, stream: 'stdout', seq: 0, data: [104, 101] },
  { log_seq: 5, stream: 'stderr', seq: 1, data: [111, 111, 112] },
  { log_seq: 8, stream: 'stdout', seq: 2, data: [108, 108, 111] },
] as const;

describe('Command.logs({ follow: true })', () => {
  let ctx: ServerContext;
  let dropAfterFirstChunk = false;
  let unsupported = false;
  const seen: Record<string, unknown>[] = [];

  beforeEach(async () => {
    seen.length = 0;
    dropAfterFirstChunk = false;
    unsupported = false;
    ctx = await startFakeServer();
    onRequest(ctx, (frame, socket) => {
      if (frame.tool !== 'sandbox_follow_logs') return;
      const payload = frame.payload as { lease_id: string; execution_id: string; after_seq?: number };
      seen.push(payload);
      respond(socket, frame, { following: true, stream_id: `logs:${payload.lease_id}:${payload.execution_id}` });
      const streamId = `logs:${payload.lease_id}:${payload.execution_id}`;
      if (unsupported) {
        send(socket, { kind: 'event', stream_id: streamId, event: { type: 'unsupported' } });
        return;
      }
      const rest = CHUNKS.filter((c) => c.log_seq > (payload.after_seq ?? 0));
      for (const chunk of rest) {
        send(socket, { kind: 'event', stream_id: streamId, event: { type: 'chunk', ...chunk, data: [...chunk.data] } });
        if (dropAfterFirstChunk) {
          setTimeout(() => socket.close(), 10);
          return;
        }
      }
      send(socket, { kind: 'event', stream_id: streamId, event: { type: 'end', next_seq: 8, truncated: false } });
    });
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  it('yields every chunk with its cursor and ends when the command exits', async () => {
    const client = await connectTestClient(ctx);
    const command = new Command(client as unknown as SandboxClientApi, leaseId('lease-1'), 'exec-1');
    const got: { stream: string; data: string; cursor: number }[] = [];
    for await (const chunk of command.logs({ follow: true })) {
      got.push({ stream: chunk.stream, data: chunk.data, cursor: chunk.cursor });
    }
    expect(got).toEqual([
      { stream: 'stdout', data: 'he', cursor: 3 },
      { stream: 'stderr', data: 'oop', cursor: 5 },
      { stream: 'stdout', data: 'llo', cursor: 8 },
    ]);
    expect(seen[0]).toMatchObject({ lease_id: 'lease-1', execution_id: 'exec-1' });
    client.close();
  });

  it('resumes after a dropped connection from the last cursor, without gaps or repeats', async () => {
    dropAfterFirstChunk = true;
    const first = await connectTestClient(ctx);
    const command = new Command(first as unknown as SandboxClientApi, leaseId('lease-1'), 'exec-1');
    const got: number[] = [];
    let failure: unknown;
    try {
      for await (const chunk of command.logs({ follow: true })) {
        got.push(chunk.cursor);
      }
    } catch (err) {
      failure = err;
    }
    expect(got).toEqual([3]);
    expect(failure).toMatchObject({ name: 'TransportError' });

    dropAfterFirstChunk = false;
    const second = await connectTestClient(ctx);
    const resumed = new Command(second as unknown as SandboxClientApi, leaseId('lease-1'), 'exec-1');
    for await (const chunk of resumed.logs({ follow: true, after: got[got.length - 1] })) {
      got.push(chunk.cursor);
    }
    expect(got).toEqual([3, 5, 8]);
    expect(seen[1]).toMatchObject({ after_seq: 3 });
    first.close();
    second.close();
  });

  it('refuses a second follow of the same command on one connection', async () => {
    const client = await connectTestClient(ctx);
    const command = new Command(client as unknown as SandboxClientApi, leaseId('lease-1'), 'exec-1');
    const one = command.logs({ follow: true })[Symbol.asyncIterator]();
    const pending = one.next();
    await expect(command.logs({ follow: true }).next()).rejects.toMatchObject({ code: 'invalid_request' });
    await pending;
    await one.return?.();
    client.close();
  });

  it('throws unsupported when the broker keeps no durable logs', async () => {
    unsupported = true;
    const client = await connectTestClient(ctx);
    const command = new Command(client as unknown as SandboxClientApi, leaseId('lease-1'), 'exec-1');
    await expect(command.logs({ follow: true }).next()).rejects.toMatchObject({ code: 'unsupported' });
    client.close();
  });

  it('keeps the buffered form without options', async () => {
    const client = new MemorySandboxClient({ commands: { hi: () => ({ exitCode: 0, stdout: 'hi' }) } });
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
    const cmd = await sandbox.runCommand({ cmd: 'hi', detached: true });
    const out: string[] = [];
    for await (const log of cmd.logs()) out.push(log.data);
    expect(out.join('')).toContain('hi');
    await expect(cmd.logs({ follow: true }).next()).rejects.toMatchObject({ code: 'unsupported' });
  });
});

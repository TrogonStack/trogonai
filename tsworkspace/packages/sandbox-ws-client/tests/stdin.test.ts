import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { StdinError } from '../src/errors.ts';
import { leaseId } from '../src/ids.ts';
import { Command } from '../src/sandbox.ts';
import {
  connectTestClient,
  onRequest,
  respond,
  respondError,
  startFakeServer,
  stopFakeServer,
  type RequestFrame,
  type ServerContext,
} from './harness.ts';

interface StdinPayload {
  lease_id: string;
  execution_id: string;
  seq: number;
  data: number[];
  eof: boolean;
}

describe('Command stdin', () => {
  let ctx: ServerContext;
  let frames: StdinPayload[];

  beforeEach(async () => {
    ctx = await startFakeServer();
    frames = [];
    clock = 0;
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  function script(reply: (payload: StdinPayload, attempt: number) => unknown | Error): void {
    onRequest(ctx, (frame: RequestFrame, socket) => {
      if (frame.tool !== 'sandbox_write_stdin') {
        return;
      }
      const payload = frame.payload as StdinPayload;
      frames.push(payload);
      const result = reply(payload, frames.length);
      if (result instanceof Error) {
        respondError(socket, frame, (result as Error & { code?: string }).code ?? 'transport', result.message);
      } else {
        respond(socket, frame, result);
      }
    });
  }

  let clock = 0;

  async function command() {
    const client = await connectTestClient(ctx);
    const backpressure = { initialDelayMs: 1, maxDelayMs: 4, budgetMs: 40, now: () => clock };
    return { client, command: new Command(client, leaseId('lease-1'), 'exec-1', false, backpressure) };
  }

  function coded(code: string): Error {
    return Object.assign(new Error(code), { code });
  }

  it('sends sandbox_write_stdin frames with gapless seq and eof on close', async () => {
    script((p) => ({ outcome: 'written', next_seq: p.seq + 1 }));
    const { client, command: cmd } = await command();
    await cmd.writeStdin('hi');
    await cmd.writeStdin(Uint8Array.from([1, 2]));
    await cmd.closeStdin();
    expect(frames).toEqual([
      { lease_id: 'lease-1', execution_id: 'exec-1', seq: 0, data: [104, 105], eof: false },
      { lease_id: 'lease-1', execution_id: 'exec-1', seq: 1, data: [1, 2], eof: false },
      { lease_id: 'lease-1', execution_id: 'exec-1', seq: 2, data: [], eof: true },
    ]);
    client.close();
  });

  it('splits writes larger than 64 KiB and puts eof on the last chunk', async () => {
    script((p) => ({ outcome: 'written', next_seq: p.seq + 1 }));
    const { client, command: cmd } = await command();
    await cmd.writeStdin(new Uint8Array(64 * 1024 + 10), { eof: true });
    expect(frames.map((f) => [f.seq, f.data.length, f.eof])).toEqual([
      [0, 65536, false],
      [1, 10, true],
    ]);
    client.close();
  });

  it('ignores empty writes without eof and is idempotent on close', async () => {
    script((p) => ({ outcome: 'written', next_seq: p.seq + 1 }));
    const { client, command: cmd } = await command();
    await cmd.writeStdin(new Uint8Array(0));
    await cmd.closeStdin();
    await cmd.closeStdin();
    expect(frames).toHaveLength(1);
    await expect(cmd.writeStdin('late')).rejects.toMatchObject({ code: 'stdin_closed' });
    client.close();
  });

  it('serializes concurrent writes in call order', async () => {
    script((p) => ({ outcome: 'written', next_seq: p.seq + 1 }));
    const { client, command: cmd } = await command();
    await Promise.all([cmd.writeStdin('a'), cmd.writeStdin('b'), cmd.writeStdin('c')]);
    expect(frames.map((f) => [f.seq, f.data[0]])).toEqual([
      [0, 97],
      [1, 98],
      [2, 99],
    ]);
    client.close();
  });

  it('retries a transport error with the same seq', async () => {
    script((p, attempt) => (attempt === 1 ? new Error('boom') : { outcome: 'written', next_seq: p.seq + 1 }));
    const { client, command: cmd } = await command();
    await cmd.writeStdin('x');
    expect(frames.map((f) => f.seq)).toEqual([0, 0]);
    client.close();
  });

  it('gives up after repeated transport errors and resumes with the same seq on the next write', async () => {
    let failing = true;
    script((p) => (failing ? new Error('down') : { outcome: 'written', next_seq: p.seq + 1 }));
    const { client, command: cmd } = await command();
    await expect(cmd.writeStdin('a')).rejects.toMatchObject({ code: 'transport' });
    const attempts = frames.length;
    failing = false;
    await cmd.writeStdin('b');
    expect(frames.slice(attempts).map((f) => [f.seq, f.data[0]])).toEqual([
      [0, 97],
      [1, 98],
    ]);
    client.close();
  });

  it('retries backpressure with the same seq until the chunk is applied', async () => {
    script((p, attempt) =>
      attempt < 4 ? { outcome: 'backpressure', next_seq: p.seq } : { outcome: 'written', next_seq: p.seq + 1 },
    );
    const { client, command: cmd } = await command();
    await cmd.writeStdin('a');
    expect(frames.map((f) => f.seq)).toEqual([0, 0, 0, 0]);
    client.close();
  });

  it('raises stdin_backpressure when the budget runs out and resumes with the same seq later', async () => {
    let pressured = true;
    script((p) => {
      clock += 15;
      return pressured ? { outcome: 'backpressure', next_seq: p.seq } : { outcome: 'written', next_seq: p.seq + 1 };
    });
    const { client, command: cmd } = await command();
    await expect(cmd.writeStdin('a')).rejects.toMatchObject({ name: 'StdinError', code: 'stdin_backpressure' });
    const attempts = frames.length;
    expect(attempts).toBe(3);
    pressured = false;
    await cmd.writeStdin('b');
    expect(frames.slice(attempts).map((f) => [f.seq, f.data[0]])).toEqual([
      [0, 97],
      [1, 98],
    ]);
    client.close();
  });

  it('retries a broker timeout', async () => {
    script((p, attempt) => (attempt === 1 ? coded('timeout') : { outcome: 'written', next_seq: p.seq + 1 }));
    const { client, command: cmd } = await command();
    await cmd.writeStdin('a');
    expect(frames.map((f) => f.seq)).toEqual([0, 0]);
    client.close();
  });

  it('does not retry an internal gateway error', async () => {
    script(() => coded('internal'));
    const { client, command: cmd } = await command();
    await expect(cmd.writeStdin('a')).rejects.toMatchObject({ code: 'internal' });
    expect(frames).toHaveLength(1);
    client.close();
  });

  it('treats already_completed on an eof chunk as success', async () => {
    script((p, attempt) => (attempt === 1 ? new Error('lost ack') : { outcome: 'already_completed' }));
    const { client, command: cmd } = await command();
    await cmd.closeStdin();
    expect(frames.map((f) => [f.seq, f.eof])).toEqual([
      [0, true],
      [0, true],
    ]);
    await expect(cmd.writeStdin('x')).rejects.toMatchObject({ code: 'stdin_closed' });
    client.close();
  });

  it('treats duplicate as success', async () => {
    script((p) => ({ outcome: 'duplicate', next_seq: p.seq + 1 }));
    const { client, command: cmd } = await command();
    await cmd.writeStdin('a');
    await cmd.writeStdin('b');
    expect(frames.map((f) => f.seq)).toEqual([0, 1]);
    client.close();
  });

  it('resends the same seq after out_of_window until the server accepts it', async () => {
    script((p, attempt) =>
      attempt === 1 ? { outcome: 'out_of_window', next_seq: p.seq } : { outcome: 'written', next_seq: p.seq + 1 },
    );
    const { client, command: cmd } = await command();
    await cmd.writeStdin('a');
    expect(frames.map((f) => f.seq)).toEqual([0, 0]);
    client.close();
  });

  it('fails when the server stops making progress', async () => {
    script((p) => ({ outcome: 'buffered', next_seq: p.seq }));
    const { client, command: cmd } = await command();
    await expect(cmd.writeStdin('a')).rejects.toMatchObject({ code: 'stdin_stalled' });
    client.close();
  });

  it('fails when the server asks for an already acknowledged seq', async () => {
    script((p) => (p.seq === 0 ? { outcome: 'written', next_seq: 1 } : { outcome: 'out_of_window', next_seq: 0 }));
    const { client, command: cmd } = await command();
    await cmd.writeStdin('a');
    await expect(cmd.writeStdin('b')).rejects.toMatchObject({ code: 'stdin_resend_unavailable' });
    client.close();
  });

  for (const outcome of [
    'stdin_closed',
    'already_completed',
    'not_found',
    'unsupported',
    'denied',
    'stdin_not_interactive',
  ]) {
    it(`treats ${outcome} as fatal for every later write`, async () => {
      script(() => ({ outcome }));
      const { client, command: cmd } = await command();
      await expect(cmd.writeStdin('a')).rejects.toBeInstanceOf(StdinError);
      const attempts = frames.length;
      await expect(cmd.writeStdin('b')).rejects.toMatchObject({ code: outcome });
      expect(frames).toHaveLength(attempts);
      client.close();
    });
  }
});

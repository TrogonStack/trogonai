import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, SandboxWsError, leaseId, tenantId } from '../src/index.ts';
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

const b64 = (s: string) => Buffer.from(s).toString('base64');

describe('Sandbox.terminal', () => {
  let ctx: ServerContext;
  let requests: RequestFrame[];

  beforeEach(async () => {
    ctx = await startFakeServer();
    requests = [];
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  function sandboxOver(client: Awaited<ReturnType<typeof connectTestClient>>): Sandbox {
    return new Sandbox(client, {
      sandboxId: leaseId('lease-1'),
      provider: 'docker' as never,
      deadline: new Date(Date.now() + 60_000),
    });
  }

  it('opens, streams output that beats the reply, forwards input and resize, and ends on exit', async () => {
    onRequest(ctx, (frame, socket) => {
      requests.push(frame);
      if (frame.tool === 'sandbox_terminal_open') {
        // The gateway is subscribed before it opens, so output can precede the reply.
        send(socket, { kind: 'event', stream_id: 't1', event: { type: 'data', seq: 0, data: b64('$ ') } });
        respond(socket, frame, { outcome: 'opened', terminal_id: 't1' });
        send(socket, { kind: 'event', stream_id: 't1', event: { type: 'data', seq: 1, data: b64('hi\r\n') } });
        send(socket, { kind: 'event', stream_id: 't1', event: { type: 'exit', seq: 2, exit_code: 3 } });
      } else {
        respond(socket, frame, { outcome: 'done' });
      }
    });
    const client = await connectTestClient(ctx);
    const term = await sandboxOver(client).terminal({ cols: 100, rows: 30 });

    await term.write('ls\n');
    await term.resize(120, 40);
    await term.close();
    await term.close();

    const text: string[] = [];
    let exit: number | undefined;
    for await (const chunk of term) {
      if (chunk.type === 'data') {
        text.push(new TextDecoder().decode(chunk.bytes));
      } else {
        exit = chunk.exitCode;
      }
    }
    expect(text.join('')).toBe('$ hi\r\n');
    expect(exit).toBe(3);

    const open = requests.find((r) => r.tool === 'sandbox_terminal_open')!;
    expect(open.payload).toEqual({ lease_id: 'lease-1', cols: 100, rows: 30 });
    const input = requests.find((r) => r.tool === 'sandbox_terminal_input')!;
    expect(input.payload).toEqual({ terminal_id: 't1', data: b64('ls\n') });
    const resize = requests.find((r) => r.tool === 'sandbox_terminal_resize')!;
    expect(resize.payload).toEqual({ terminal_id: 't1', cols: 120, rows: 40 });
    expect(requests.filter((r) => r.tool === 'sandbox_terminal_close')).toHaveLength(1);
    client.close();
  });

  it('yields a resync chunk instead of mistaking it for the end of the terminal', async () => {
    onRequest(ctx, (frame, socket) => {
      if (frame.tool === 'sandbox_terminal_open') {
        respond(socket, frame, { outcome: 'opened', terminal_id: 't1' });
        send(socket, { kind: 'event', stream_id: 't1', event: { type: 'resync', from_seq: 7 } });
        send(socket, { kind: 'event', stream_id: 't1', event: { type: 'data', seq: 7, data: b64('$ ') } });
        send(socket, { kind: 'event', stream_id: 't1', event: { type: 'exit', seq: 8 } });
      } else {
        respond(socket, frame, { outcome: 'done' });
      }
    });
    const client = await connectTestClient(ctx);
    const term = await sandboxOver(client).terminal();
    const types: string[] = [];
    let fromSeq: number | undefined;
    for await (const chunk of term) {
      types.push(chunk.type);
      if (chunk.type === 'resync') {
        fromSeq = chunk.fromSeq;
      }
    }
    expect(types).toEqual(['resync', 'data', 'exit']);
    expect(fromSeq).toBe(7);
    client.close();
  });

  it('surfaces an unsupported provider as a typed error', async () => {
    onRequest(ctx, (frame, socket) => {
      respondError(socket, frame, 'unsupported', 'the provider does not support interactive terminals');
    });
    const client = await connectTestClient(ctx);
    await expect(sandboxOver(client).terminal()).rejects.toBeInstanceOf(SandboxWsError);
    client.close();
  });

  it('the in-memory client refuses terminals instead of faking one', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
    await expect(sandbox.terminal()).rejects.toBeInstanceOf(SandboxWsError);
  });
});

import type { AddressInfo } from 'node:net';

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { WebSocket as NodeWebSocket, WebSocketServer } from 'ws';

import { SandboxWsClient } from '../src/client.ts';
import { LEASE_ERROR_CLASSES, errorFromCode, MemorySandboxClient } from '../src/index.ts';
import type { WsFrame } from '../src/protocol.ts';

const ENTITY_ID = '01HZZZZZZZZZZZZZZZZZZZZZZZ';

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

async function connectTestClient(ctx: ServerContext): Promise<SandboxWsClient> {
  const ClientCtor = SandboxWsClient as unknown as { new (socket: WebSocket): SandboxWsClient };
  const socket = new NodeWebSocket(`${ctx.url}/v1/ws`, ['bearer.test-token']) as unknown as WebSocket;
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve());
    socket.addEventListener('error', (event: Event) => reject(event));
  });
  return new ClientCtor(socket);
}

/** Answers every request with `result` and records the frames it saw. */
function answerWith(ctx: ServerContext, result: unknown): WsFrame[] {
  const seen: WsFrame[] = [];
  ctx.wss.on('connection', (socket) => {
    socket.on('message', (raw) => {
      const frame = JSON.parse(String(raw)) as WsFrame;
      if (frame.kind === 'request') {
        seen.push(frame);
        socket.send(JSON.stringify({ kind: 'response', id: frame.id, result }));
      }
    });
  });
  return seen;
}

describe('entity verbs', () => {
  let ctx: ServerContext;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    await new Promise<void>((resolve) => ctx.wss.close(() => resolve()));
  });

  it('sends the entity selector as entity_id or name', async () => {
    const seen = answerWith(ctx, { outcome: 'not_found' });
    const client = await connectTestClient(ctx);
    expect(await client.getEntity({ name: 'repl-1' })).toEqual({ outcome: 'not_found' });
    expect(await client.deleteEntity({ id: ENTITY_ID })).toEqual({ outcome: 'not_found' });
    expect(seen.map((f) => (f.kind === 'request' ? [f.tool, f.payload] : null))).toEqual([
      ['sandbox_get_entity', { name: 'repl-1' }],
      ['sandbox_delete_entity', { entity_id: ENTITY_ID }],
    ]);
    client.close();
  });

  it('pages a listing with limit and cursor only when given', async () => {
    const seen = answerWith(ctx, { outcome: 'listed', rows: [] });
    const client = await connectTestClient(ctx);
    expect(await client.listEntities()).toEqual({ outcome: 'listed', rows: [] });
    await client.listEntities({ limit: 5, cursor: ENTITY_ID });
    expect(seen.map((f) => (f.kind === 'request' ? f.payload : null))).toEqual([{}, { limit: 5, cursor: ENTITY_ID }]);
    client.close();
  });

  it('classifies the two entity errors', () => {
    const unavailable = errorFromCode('lease_denied', 'store down', { kind: 'entity_unavailable' });
    expect(unavailable.retryable).toBe(true);
    expect(LEASE_ERROR_CLASSES.entity_unavailable.retryable).toBe(true);
    const off = errorFromCode('lease_denied', 'off', { kind: 'entity_not_enabled' });
    expect(off.retryable).toBe(false);
    expect(off.variant).toBe('entity_not_enabled');
  });

  it('keeps no entities in the in-memory client', async () => {
    const client = new MemorySandboxClient();
    expect(await client.getEntity({ name: 'x' })).toEqual({ outcome: 'not_found' });
    expect(await client.listEntities()).toEqual({ outcome: 'listed', rows: [] });
    expect(await client.deleteEntity({ id: ENTITY_ID })).toEqual({ outcome: 'not_found' });
  });
});

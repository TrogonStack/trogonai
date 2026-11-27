import type { AddressInfo } from 'node:net';

import type { WsFrame } from '@trogonai/sandbox-ws-client';
import { WebSocketServer } from 'ws';

export interface FakeGateway {
  wss: WebSocketServer;
  wsUrl: string;
  httpUrl: string;
  originalFetch: typeof fetch;
}

function send(socket: import('ws').WebSocket, frame: WsFrame): void {
  socket.send(JSON.stringify(frame));
}

/**
 * Spins up a real `ws` WebSocketServer (mirroring the @trogonai/sandbox-ws-client test
 * pattern) and stubs `global.fetch` to satisfy `SandboxWsClient.connect()`'s
 * HTTP session-creation POST to `${gatewayHttpUrl}/v1/sessions`, so the CLI
 * can drive a full `SandboxWsClient.connect()` -> WS round trip in-process.
 */
export function startFakeGateway(): Promise<FakeGateway> {
  return new Promise((resolve) => {
    const wss = new WebSocketServer({ port: 0, host: '127.0.0.1' });
    wss.on('listening', () => {
      const { port } = wss.address() as AddressInfo;
      const wsUrl = `ws://127.0.0.1:${port}`;
      const httpUrl = `http://127.0.0.1:${port}`;
      const originalFetch = globalThis.fetch;
      globalThis.fetch = (async (url: string) => {
        if (String(url).endsWith('/v1/sessions')) {
          return new Response(JSON.stringify({ token: 'test-token', expires_at: 9999999999 }), { status: 201 });
        }
        throw new Error(`unexpected fetch in test: ${url}`);
      }) as unknown as typeof fetch;
      resolve({ wss, wsUrl, httpUrl, originalFetch });
    });
  });
}

export function stopFakeGateway(gateway: FakeGateway): Promise<void> {
  globalThis.fetch = gateway.originalFetch;
  return new Promise((resolve) => {
    gateway.wss.close(() => resolve());
  });
}

export function onRequest(
  gateway: FakeGateway,
  tool: string,
  handler: (frame: Extract<WsFrame, { kind: 'request' }>, socket: import('ws').WebSocket) => void,
): void {
  gateway.wss.on('connection', (socket) => {
    socket.on('message', (raw) => {
      const frame = JSON.parse(String(raw)) as WsFrame;
      if (frame.kind === 'request' && frame.tool === tool) {
        handler(frame, socket);
      }
    });
  });
}

export function sendResult(socket: import('ws').WebSocket, id: string, result: unknown): void {
  send(socket, { kind: 'response', id, result });
}

export function baseArgv(gateway: FakeGateway, extra: string[]): string[] {
  return [
    'node',
    'sandbox',
    '--gateway-http',
    gateway.httpUrl,
    '--gateway-ws',
    gateway.wsUrl,
    '--api-key',
    'test-key',
    '--tenant',
    'tenant-a',
    ...extra,
  ];
}

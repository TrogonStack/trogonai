import type { AddressInfo } from 'node:net';

import { WebSocket as NodeWebSocket, WebSocketServer } from 'ws';

import { SandboxWsClient } from '../src/client.ts';
import type { WsFrame } from '../src/protocol.ts';

export interface ServerContext {
  wss: WebSocketServer;
  url: string;
}

export function startFakeServer(): Promise<ServerContext> {
  return new Promise((resolve) => {
    // Bind the address the URL names. A bare port 0 binds the IPv6 wildcard, where the OS can
    // hand out a port that another process already holds on 127.0.0.1, so the client then
    // reaches that process instead of this server.
    const wss = new WebSocketServer({ port: 0, host: '127.0.0.1' });
    wss.on('listening', () => {
      const { port } = wss.address() as AddressInfo;
      resolve({ wss, url: `ws://127.0.0.1:${port}` });
    });
  });
}

export function stopFakeServer(ctx: ServerContext): Promise<void> {
  return new Promise((resolve) => {
    ctx.wss.close(() => resolve());
  });
}

export function send(socket: NodeWebSocket, frame: WsFrame): void {
  socket.send(JSON.stringify(frame));
}

export async function connectTestClient(ctx: ServerContext): Promise<SandboxWsClient> {
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

export type RequestFrame = Extract<WsFrame, { kind: 'request' }>;

export function onRequest(ctx: ServerContext, handler: (frame: RequestFrame, socket: NodeWebSocket) => void): void {
  ctx.wss.on('connection', (socket) => {
    socket.on('message', (raw) => {
      const frame = JSON.parse(String(raw)) as WsFrame;
      if (frame.kind === 'request') {
        handler(frame, socket);
      }
    });
  });
}

export function respond(socket: NodeWebSocket, frame: RequestFrame, result: unknown): void {
  send(socket, { kind: 'response', id: frame.id, result });
}

export function respondError(socket: NodeWebSocket, frame: RequestFrame, code: string, message: string): void {
  send(socket, { kind: 'response', id: frame.id, error: { code, message } });
}

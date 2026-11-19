import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';

import { afterEach, describe, expect, it } from 'vitest';

import { SandboxWsClient, TransportError } from '../src/index.ts';
import { startFakeServer, stopFakeServer } from './harness.ts';
import type { ServerContext } from './harness.ts';

let ctx: ServerContext | undefined;

afterEach(async () => {
  if (ctx) {
    await stopFakeServer(ctx);
    ctx = undefined;
  }
});

describe('SandboxWsClient.connectWithToken', () => {
  it('opens the socket with the token as the bearer subprotocol and never calls createSession', async () => {
    ctx = await startFakeServer();
    const protocols: string[] = [];
    ctx.wss.on('connection', (_socket, request) => {
      protocols.push(String(request.headers['sec-websocket-protocol']));
    });
    const realFetch = globalThis.fetch;
    let fetched = 0;
    globalThis.fetch = (async () => {
      fetched += 1;
      throw new Error('createSession must not run');
    }) as typeof fetch;
    try {
      const client = await SandboxWsClient.connectWithToken({ gatewayWsUrl: ctx.url, token: 'minted-by-backend' });
      expect(protocols).toEqual(['bearer.minted-by-backend']);
      expect(fetched).toBe(0);
      client.close();
    } finally {
      globalThis.fetch = realFetch;
    }
  });

  it('rejects malformed options before opening a socket', async () => {
    await expect(SandboxWsClient.connectWithToken({ gatewayWsUrl: 'http://gw', token: 't' })).rejects.toBeInstanceOf(
      TransportError,
    );
    await expect(SandboxWsClient.connectWithToken({ gatewayWsUrl: 'ws://gw', token: '' })).rejects.toThrow(
      /token: must not be empty/,
    );
  });

  it('maps a rejected handshake to a transport error', async () => {
    const server = createServer();
    server.on('upgrade', (_request, socket) => {
      socket.end('HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n');
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const { port } = server.address() as AddressInfo;
    try {
      await expect(
        SandboxWsClient.connectWithToken({ gatewayWsUrl: `ws://127.0.0.1:${port}`, token: 'expired' }),
      ).rejects.toBeInstanceOf(TransportError);
    } finally {
      await new Promise((resolve) => server.close(resolve));
    }
  });
});

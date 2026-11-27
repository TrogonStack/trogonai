import assert from 'node:assert/strict';
import { test } from 'node:test';

import { MemorySandboxClient, SandboxWsClient } from '@trogonai/sandbox-ws-client';

import { sharedClient } from './sandbox.ts';

for (const provider of ['', 'vercel', 'not-a-provider']) {
  test('rejects incompatible provider configuration before connecting', async (context) => {
    const names = ['SANDBOX_PROVIDER', 'SANDBOX_GATEWAY_HTTP_URL', 'SANDBOX_GATEWAY_WS_URL', 'SANDBOX_API_KEY'];
    const previous = new Map(names.map((name) => [name, process.env[name]]));
    process.env.SANDBOX_PROVIDER = provider;
    process.env.SANDBOX_GATEWAY_HTTP_URL = 'http://gateway.invalid';
    process.env.SANDBOX_GATEWAY_WS_URL = 'ws://gateway.invalid';
    process.env.SANDBOX_API_KEY = 'test-only-placeholder';
    context.after(() => {
      for (const [name, value] of previous) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
    });
    const cache = (globalThis as { __honoExample?: { client: unknown } }).__honoExample;
    if (cache !== undefined) cache.client = undefined;
    let connected = 0;
    context.mock.method(SandboxWsClient, 'connect', async () => {
      connected++;
      return new MemorySandboxClient();
    });
    await assert.rejects(async () => sharedClient(), /SANDBOX_PROVIDER|provider/);
    assert.equal(connected, 0);
  });
}

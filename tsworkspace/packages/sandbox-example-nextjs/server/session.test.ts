import assert from 'node:assert/strict';
import { test } from 'node:test';

import { mintBrowserSession } from './session.ts';

for (const body of [
  { expires_at: 2_000_000_000 },
  { token: '', expires_at: 2_000_000_000 },
  { token: 'test-only-session', expires_at: 'forever' },
  { token: 'test-only-session', expires_at: -1 },
]) {
  test('browser session mint rejects an invalid gateway response', async (context) => {
    const names = ['SANDBOX_PROVIDER', 'SANDBOX_GATEWAY_HTTP_URL', 'SANDBOX_GATEWAY_WS_URL', 'SANDBOX_API_KEY'];
    const previous = new Map(names.map((name) => [name, process.env[name]]));
    process.env.SANDBOX_PROVIDER = 'local_docker';
    process.env.SANDBOX_GATEWAY_HTTP_URL = 'http://gateway.invalid';
    process.env.SANDBOX_GATEWAY_WS_URL = 'ws://gateway.invalid';
    process.env.SANDBOX_API_KEY = 'test-only-placeholder';
    context.after(() => {
      for (const [name, value] of previous) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
    });
    context.mock.method(globalThis, 'fetch', async () => Response.json(body, { status: 201 }));
    await assert.rejects(mintBrowserSession, /token|expires_at|session/i);
  });
}

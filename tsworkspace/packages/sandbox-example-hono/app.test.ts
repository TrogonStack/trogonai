import assert from 'node:assert/strict';
import { test } from 'node:test';

import { app } from './app.ts';

test('run rejects a blank script', async (context) => {
  const previous = process.env.SANDBOX_PROVIDER;
  process.env.SANDBOX_PROVIDER = 'memory';
  context.after(() => {
    if (previous === undefined) delete process.env.SANDBOX_PROVIDER;
    else process.env.SANDBOX_PROVIDER = previous;
  });
  const response = await app.request('/run', {
    method: 'POST',
    body: JSON.stringify({ script: '  \n  ' }),
    headers: { 'Content-Type': 'application/json' },
  });
  assert.equal(response.status, 400);
});

for (const query of ['maxAgeSeconds=', 'dryRun=maybe']) {
  test('reaper rejects ambiguous query options', async (context) => {
    const previous = new Map(['SANDBOX_PROVIDER', 'ADMIN_TOKEN'].map((name) => [name, process.env[name]]));
    process.env.SANDBOX_PROVIDER = 'memory';
    process.env.ADMIN_TOKEN = 'test-only-placeholder';
    context.after(() => {
      for (const [name, value] of previous) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
    });
    const response = await app.request('/admin/reap?' + query, {
      method: 'POST',
      headers: { authorization: 'Bearer test-only-placeholder' },
    });
    assert.equal(response.status, 400);
  });
}

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { runScript } from './actions.ts';
import { GET as reap } from './api/reap/route.ts';
import { POST as run } from './api/run/route.ts';

test('run rejects a blank script', async (context) => {
  const previous = process.env.SANDBOX_PROVIDER;
  process.env.SANDBOX_PROVIDER = 'memory';
  context.after(() => {
    if (previous === undefined) delete process.env.SANDBOX_PROVIDER;
    else process.env.SANDBOX_PROVIDER = previous;
  });
  const response = await run(
    new Request('http://example.invalid/api/run', {
      method: 'POST',
      body: JSON.stringify({ script: '  \n  ' }),
    }),
  );
  assert.equal(response.status, 400);
});

test('server action rejects a file instead of coercing it into a shell script', async (context) => {
  const previous = process.env.SANDBOX_PROVIDER;
  process.env.SANDBOX_PROVIDER = 'memory';
  context.after(() => {
    if (previous === undefined) delete process.env.SANDBOX_PROVIDER;
    else process.env.SANDBOX_PROVIDER = previous;
  });
  const form = new FormData();
  form.set('script', new File(['echo file'], 'script.sh'));
  assert.equal((await runScript(form)).exitCode, -1);
});

for (const query of ['maxAgeSeconds=', 'dryRun=maybe']) {
  test('reaper rejects ambiguous query options', async (context) => {
    const previous = new Map(['SANDBOX_PROVIDER', 'CRON_SECRET'].map((name) => [name, process.env[name]]));
    process.env.SANDBOX_PROVIDER = 'memory';
    process.env.CRON_SECRET = 'test-only-placeholder';
    context.after(() => {
      for (const [name, value] of previous) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
    });
    const response = await reap(
      new Request('http://example.invalid/api/reap?' + query, {
        headers: { authorization: 'Bearer test-only-placeholder' },
      }),
    );
    assert.equal(response.status, 400);
  });
}

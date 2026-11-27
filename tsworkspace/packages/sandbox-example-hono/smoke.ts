// Calls every route through `app.request()`. Run with `pnpm smoke` (memory sandbox, no gateway).
import assert from 'node:assert/strict';

import { Sandbox, tenantId } from '@trogonai/sandbox-ws-client';

import { app } from './app.ts';
import { sharedClient } from './server/sandbox.ts';

assert.equal(process.env.SANDBOX_PROVIDER, 'memory', 'run through `pnpm smoke`');
process.env.ADMIN_TOKEN = 'smoke-admin-token';

const post = (path: string, body?: unknown, headers?: Record<string, string>) =>
  app.request(path, { method: 'POST', body: body === undefined ? undefined : JSON.stringify(body), headers });
const activeLeases = async (): Promise<number> => (await (await sharedClient()).listLeases()).leases.length;

assert.deepEqual(await (await app.request('/health')).json(), { ok: true, mode: 'memory' });

// A run returns its output and releases the lease; a non-zero exit code is a result.
const ok = await post('/run', { script: 'echo hello' });
assert.equal(ok.status, 200);
assert.deepEqual(await ok.json(), { exitCode: 0, stdout: 'memory mode, not executed: echo hello\n', stderr: '' });
assert.equal(await activeLeases(), 0, 'the run released its lease');
assert.equal(((await (await post('/run', { script: 'exit 3' })).json()) as { exitCode: number }).exitCode, 3);
assert.equal((await post('/run', {})).status, 400);
assert.equal((await app.request('/run', { method: 'POST', body: 'not json' })).status, 400);

// The streamed route sends one JSON event per line, ending with the exit code.
const streamed = await post('/run/stream', { script: 'echo hello' });
assert.equal(streamed.status, 200);
assert.match(streamed.headers.get('content-type') ?? '', /application\/x-ndjson/);
const events = (await streamed.text())
  .trimEnd()
  .split('\n')
  .map((line) => JSON.parse(line) as { type: string });
assert.deepEqual(events, [
  { type: 'output', stream: 'stdout', data: 'memory mode, not executed: echo hello\n' },
  { type: 'exit', exitCode: 0 },
]);
assert.equal(await activeLeases(), 0, 'the stream released its lease');
assert.equal((await post('/run/stream', {})).status, 400);

assert.equal((await post('/api/sandbox-session')).status, 501);

const capacity = (await (await app.request('/capacity')).json()) as { available: boolean };
assert.equal(capacity.available, true);

// Reaper: refuses callers without the bearer token, then releases an aged lease.
const auth = { authorization: `Bearer ${process.env.ADMIN_TOKEN}` };
assert.equal((await post('/admin/reap')).status, 401);
assert.equal((await post('/admin/reap?maxAgeSeconds=-1', undefined, auth)).status, 400);

const stale = await Sandbox.create(await sharedClient(), { tenant: tenantId('memory'), timeoutMs: 60_000 });
assert.equal(await activeLeases(), 1);
const young = (await (await post('/admin/reap', undefined, auth)).json()) as { matched: number };
assert.equal(young.matched, 0, 'a fresh lease is younger than the default cutoff');
const dry = (await (await post('/admin/reap?maxAgeSeconds=0&dryRun=1', undefined, auth)).json()) as {
  dryRun: boolean;
  matched: number;
};
assert.deepEqual([dry.dryRun, dry.matched], [true, 1]);
assert.equal(await activeLeases(), 1, 'a dry run releases nothing');
const reaped = (await (await post('/admin/reap?maxAgeSeconds=0', undefined, auth)).json()) as { released: string[] };
assert.deepEqual(reaped.released, [stale.sandboxId]);
assert.equal(await activeLeases(), 0);

console.log('smoke ok: health, run, run/stream, sandbox-session, capacity, admin/reap');

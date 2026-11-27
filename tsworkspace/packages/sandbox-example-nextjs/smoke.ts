// Calls every route handler and the server action in-process. Run with `pnpm smoke` (memory sandbox, no gateway).
import assert from 'node:assert/strict';

import { Sandbox, tenantId } from '@trogonai/sandbox-ws-client';

import { runScript } from './app/actions.ts';
import { GET as reap } from './app/api/reap/route.ts';
import { POST as run } from './app/api/run/route.ts';
import { POST as mintSession } from './app/api/sandbox-session/route.ts';
import { sharedClient } from './server/sandbox.ts';

assert.equal(process.env.SANDBOX_PROVIDER, 'memory', 'run through `pnpm smoke`');
process.env.CRON_SECRET = 'smoke-cron-secret';

const json = (body: unknown): RequestInit => ({ method: 'POST', body: JSON.stringify(body) });
const activeLeases = async (): Promise<number> => {
  const { leases } = await (await sharedClient()).listLeases();
  return leases.length;
};

// Route handler: a run returns its output and releases the lease.
const ok = await run(new Request('http://localhost/api/run', json({ script: 'echo hello' })));
assert.equal(ok.status, 200);
assert.deepEqual(await ok.json(), { exitCode: 0, stdout: 'memory mode, not executed: echo hello\n', stderr: '' });
assert.equal(await activeLeases(), 0, 'the run released its lease');

// A non-zero exit code is a result, not an error.
const failed = await run(new Request('http://localhost/api/run', json({ script: 'exit 3' })));
assert.equal(failed.status, 200);
assert.equal(((await failed.json()) as { exitCode: number }).exitCode, 3);

// Bad input is rejected before a sandbox is leased.
assert.equal((await run(new Request('http://localhost/api/run', json({})))).status, 400);
assert.equal((await run(new Request('http://localhost/api/run', { method: 'POST', body: 'not json' }))).status, 400);

// Server action: the same module behind a form.
const form = new FormData();
form.set('script', 'echo from the action');
assert.deepEqual(await runScript(form), {
  exitCode: 0,
  stdout: 'memory mode, not executed: echo from the action\n',
  stderr: '',
});
assert.equal((await runScript(new FormData())).exitCode, -1);

// Token route: memory mode has no gateway, so it says so.
assert.equal((await mintSession()).status, 501);

// Reaper: refuses callers without the bearer token, then releases an aged lease.
const reapUrl = 'http://localhost/api/reap';
const auth = { headers: { authorization: `Bearer ${process.env.CRON_SECRET}` } };
assert.equal((await reap(new Request(reapUrl))).status, 401);
assert.equal((await reap(new Request(reapUrl, { headers: { authorization: 'Bearer wrong' } }))).status, 401);
assert.equal((await reap(new Request(`${reapUrl}?maxAgeSeconds=-1`, auth))).status, 400);

const stale = await Sandbox.create(await sharedClient(), { tenant: tenantId('memory'), timeoutMs: 60_000 });
assert.equal(await activeLeases(), 1);

const young = (await (await reap(new Request(reapUrl, auth))).json()) as { matched: number };
assert.equal(young.matched, 0, 'a fresh lease is younger than the default cutoff');

const dry = (await (await reap(new Request(`${reapUrl}?maxAgeSeconds=0&dryRun=1`, auth))).json()) as {
  dryRun: boolean;
  matched: number;
  released: string[];
};
assert.deepEqual([dry.dryRun, dry.matched, dry.released.length], [true, 1, 0]);
assert.equal(await activeLeases(), 1, 'a dry run releases nothing');

const reaped = (await (await reap(new Request(`${reapUrl}?maxAgeSeconds=0`, auth))).json()) as { released: string[] };
assert.deepEqual(reaped.released, [stale.sandboxId]);
assert.equal(await activeLeases(), 0);

console.log('smoke ok: run, server action, sandbox-session, reap');

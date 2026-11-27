import assert from 'node:assert/strict';
import { test } from 'node:test';

import { Sandbox, SandboxWsClient, leaseId, tenantId } from '@trogonai/sandbox-ws-client';
import type { LeaseId, LeaseListRow } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

import { app } from './app.ts';
import { runResultSchema, streamEventSchema } from './server/contracts.ts';
import { sharedClient } from './server/sandbox.ts';

test(
  'Hono executes, streams and mints a browser session through a real local gateway',
  { timeout: 120_000 },
  async (context) => {
    const config = z
      .object({
        SANDBOX_PROVIDER: z.literal('local_docker').default('local_docker'),
        SANDBOX_GATEWAY_HTTP_URL: z.string().url(),
        SANDBOX_GATEWAY_WS_URL: z.string().url(),
        SANDBOX_API_KEY: z.string().min(1),
        SANDBOX_TENANT: z.string().min(1),
      })
      .parse(process.env);
    const client = await sharedClient();
    assert.ok(client instanceof SandboxWsClient);
    const owned = new Set<LeaseId>();
    const lease = client.lease.bind(client);
    context.mock.method(client, 'lease', async (...args: Parameters<typeof client.lease>) => {
      const grant = await lease(...args);
      owned.add(grant.lease_id);
      return grant;
    });
    let cancelStream: (() => Promise<void>) | undefined;
    context.after(async () => {
      try {
        const cancellation = await Promise.allSettled([cancelStream?.()]);
        const present = (await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id)));
        const results = await Promise.allSettled(present.map((row) => client.deleteLease(leaseId(row.lease_id))));
        const failures = [...cancellation, ...results].filter((result) => result.status === 'rejected');
        assert.equal(failures.length, 0, 'every remaining observed lease must be cleaned up');
        assert.equal((await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id))).length, 0);
      } finally {
        client.close();
      }
    });

    const run = await app.request('/run', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ script: 'printf hono-gateway-proof' }),
    });
    assert.equal(run.status, 200);
    assert.equal(runResultSchema.parse(await run.json()).stdout, 'hono-gateway-proof');

    const releasePath = '/tmp/hono-stream-' + crypto.randomUUID();
    const response = await app.request('/run/stream', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        script:
          'printf "stream-before\\n"; while [ ! -f ' + releasePath + ' ]; do sleep 0.1; done; printf "stream-after\\n"',
      }),
    });
    assert.equal(response.status, 200);
    assert.ok(response.body);
    const reader = response.body.getReader();
    cancelStream = () => reader.cancel();
    const decoder = new TextDecoder();
    let pending = '';
    let output = '';
    let released = false;
    let exitCode: number | undefined;
    for (;;) {
      const page = await reader.read();
      if (page.done) break;
      pending += decoder.decode(page.value, { stream: true });
      let end: number;
      while ((end = pending.indexOf('\n')) >= 0) {
        const line = pending.slice(0, end);
        pending = pending.slice(end + 1);
        if (line === '') continue;
        const event = streamEventSchema.parse(JSON.parse(line));
        assert.notEqual(event.type, 'error');
        if (event.type === 'output') {
          output += event.data;
          if (!released && output.includes('stream-before')) {
            const added: LeaseListRow[] = (await client.listLeases()).leases.filter((row) =>
              owned.has(leaseId(row.lease_id)),
            );
            assert.equal(added.length, 1, 'the stream must have one active observed lease');
            await client.writeFiles(leaseId(added[0]!.lease_id), [
              { path: releasePath, content: [...new TextEncoder().encode('release')] },
            ]);
            released = true;
          }
        } else if (event.type === 'exit') {
          exitCode = event.exitCode;
        }
      }
    }
    assert.equal(released, true);
    assert.equal(exitCode, 0);
    assert.match(output, /stream-before/);
    assert.match(output, /stream-after/);

    const mint = await app.request('/api/sandbox-session', { method: 'POST' });
    assert.equal(mint.status, 200);
    const grant = z.object({ token: z.string().min(1), gatewayWsUrl: z.string().url() }).parse(await mint.json());
    const browser = await SandboxWsClient.connectWithToken(grant);
    try {
      const sandbox = await Sandbox.create(browser, {
        tenant: tenantId(config.SANDBOX_TENANT),
        providerPref: ['local_docker'],
        timeoutMs: 60_000,
      });
      owned.add(sandbox.sandboxId);
      try {
        const result = await sandbox.runCommand('sh', ['-c', 'printf browser-session-proof']);
        assert.equal(result.stdout(), 'browser-session-proof');
      } finally {
        await sandbox.delete();
      }
    } finally {
      browser.close();
    }
    const leaked = (await client.listLeases()).leases.filter((lease) => owned.has(leaseId(lease.lease_id)));
    assert.equal(leaked.length, 0, 'the flow must release every lease it created');
  },
);

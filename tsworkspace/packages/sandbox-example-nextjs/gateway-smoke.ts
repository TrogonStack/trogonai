import assert from 'node:assert/strict';
import { test } from 'node:test';

import { Sandbox, SandboxWsClient, leaseId, tenantId } from '@trogonai/sandbox-ws-client';
import type { LeaseId } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

import { runScript } from './app/actions.ts';
import { POST as run } from './app/api/run/route.ts';
import { POST as mintSession } from './app/api/sandbox-session/route.ts';
import { runResultSchema } from './server/contracts.ts';
import { sharedClient } from './server/sandbox.ts';

test(
  'Next routes and actions execute and mint a browser session through a real local gateway',
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
    context.after(async () => {
      try {
        const present = (await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id)));
        const results = await Promise.allSettled(present.map((row) => client.deleteLease(leaseId(row.lease_id))));
        assert.ok(
          results.every((result) => result.status === 'fulfilled'),
          'every remaining observed lease must be cleaned up',
        );
        assert.equal((await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id))).length, 0);
      } finally {
        client.close();
      }
    });

    const response = await run(
      new Request('http://example.invalid/api/run', {
        method: 'POST',
        body: JSON.stringify({ script: 'printf next-route-proof' }),
      }),
    );
    assert.equal(response.status, 200);
    assert.equal(runResultSchema.parse(await response.json()).stdout, 'next-route-proof');
    const form = new FormData();
    form.set('script', 'printf next-action-proof');
    assert.equal((await runScript(form)).stdout, 'next-action-proof');

    const mint = await mintSession();
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
        assert.equal(
          (await sandbox.runCommand('sh', ['-c', 'printf next-browser-proof'])).stdout(),
          'next-browser-proof',
        );
      } finally {
        await sandbox.delete();
      }
    } finally {
      browser.close();
    }
    const leaked = (await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id)));
    assert.equal(leaked.length, 0, 'the flow must release every lease it created');
  },
);

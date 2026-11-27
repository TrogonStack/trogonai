import assert from 'node:assert/strict';
import { fork } from 'node:child_process';
import { test } from 'node:test';

import { SandboxWsClient, leaseId } from '@trogonai/sandbox-ws-client';
import type { LeaseId } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

import { createGitFixture } from './test-support/git-fixture.ts';

const grantSchema = z.object({ type: z.literal('grant'), leaseId: z.string().min(1) });

test(
  'coding CLI shallow-clones, edits and reports a real Git diff through the local gateway',
  { timeout: 120_000 },
  async (context) => {
    const config = z
      .object({
        SANDBOX_GATEWAY_HTTP_URL: z.string().url(),
        SANDBOX_GATEWAY_WS_URL: z.string().url(),
        SANDBOX_API_KEY: z.string().min(1),
        SANDBOX_TENANT: z.string().min(1),
        SANDBOX_GIT_FIXTURE_HOST: z.string().optional(),
      })
      .parse(process.env);
    const fixture = await createGitFixture({ dockerHost: config.SANDBOX_GIT_FIXTURE_HOST });
    let client: SandboxWsClient;
    try {
      client = await SandboxWsClient.connect({
        gatewayHttpUrl: config.SANDBOX_GATEWAY_HTTP_URL,
        gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
        apiKey: config.SANDBOX_API_KEY,
      });
    } catch (error) {
      await fixture.close();
      throw error;
    }
    const owned = new Set<LeaseId>();
    context.after(async () => {
      try {
        const cleanupLeases = async () => {
          const present = (await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id)));
          const removed = await Promise.allSettled(present.map((row) => client.deleteLease(leaseId(row.lease_id))));
          assert.ok(removed.every((result) => result.status === 'fulfilled'));
          assert.equal((await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id))).length, 0);
        };
        const results = await Promise.allSettled([cleanupLeases(), fixture.close()]);
        assert.ok(
          results.every((result) => result.status === 'fulfilled'),
          'clean up the remaining observed leases and owned Git fixture',
        );
      } finally {
        client.close();
      }
    });
    const flags = process.allowedNodeEnvironmentFlags.has('--experimental-transform-types')
      ? ['--experimental-transform-types']
      : [];
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      SANDBOX_PROVIDER: 'local_docker',
      SANDBOX_EXAMPLE_MODEL: 'scripted',
    };
    delete env.GIT_TOKEN;
    delete env.SANDBOX_EXAMPLE_MODEL_API_KEY;
    const child = fork(new URL('./index.ts', import.meta.url), ['Greet the world', '--repo', fixture.dockerUrl], {
      env,
      execArgv: [...flags, '--import', new URL('./test-support/observe-gateway.ts', import.meta.url).href],
      stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
    });
    context.after(() => {
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
    });
    let stdout = '';
    let stderr = '';
    child.stdout!.on('data', (chunk: Buffer) => {
      stdout += chunk.toString();
    });
    child.stderr!.on('data', (chunk: Buffer) => {
      stderr += chunk.toString();
    });
    child.on('message', (message) => {
      owned.add(leaseId(grantSchema.parse(message).leaseId));
    });
    const result = await new Promise<{ code: number | null; signal: string | null }>((resolve, reject) => {
      child.once('error', reject);
      child.once('close', (code, signal) => resolve({ code, signal }));
    });
    assert.equal(result.signal, null);
    assert.equal(result.code, 0, stderr);
    assert.equal(owned.size, 1);
    assert.match(stdout, /diff --git a\/greeting.txt b\/greeting.txt/);
    assert.match(stdout, /-Hello, there!/);
    assert.match(stdout, /\+Hello, world!/);
    assert.equal((await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id))).length, 0);
  },
);

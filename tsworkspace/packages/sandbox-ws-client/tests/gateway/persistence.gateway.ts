import assert from 'node:assert/strict';
import { test } from 'node:test';

import { z } from 'zod';

import type { LeaseId, Sandbox as SandboxHandle, SandboxWsClient as Client } from '../../src/index.ts';

const { Sandbox, SandboxWsClient, leaseId, serverClientStateSchema, tenantId }: typeof import('../../src/index.ts') =
  await import(new URL('../../dist/index.js', import.meta.url).href);

const markerSchema = z.strictObject({
  marker: z.string().uuid(),
  pid: z.number().int().positive(),
  start: z.string().regex(/^\d+$/),
  launches: z.number().int().positive(),
});
const commandOutputSchema = z.object({
  exitCode: z.number().int(),
  stdout: z.string(),
  stderr: z.string(),
  timedOut: z.boolean(),
  truncated: z.boolean(),
});
const attachmentSchema = z.object({
  sandboxId: z.string().min(1),
  provider: z.literal('local_docker'),
  name: z.string().min(1),
  tags: z.record(z.string(), z.string()),
  state: z.enum(['running', 'paused']),
  created: z.literal(false),
  createdAt: z.date(),
});

test(
  'named Docker servers survive fresh clients and pause/resume without duplicate launches',
  { timeout: 120_000 },
  async (context) => {
    const config = z
      .object({
        SANDBOX_GATEWAY_HTTP_URL: z.string().url(),
        SANDBOX_GATEWAY_WS_URL: z.string().url(),
        SANDBOX_API_KEY: z.string().min(1),
        SANDBOX_TENANT: z.string().min(1),
      })
      .parse(process.env);
    const owned = new Set<LeaseId>();
    const connections = new Set<Client>();
    async function connect(): Promise<Client> {
      const client = await SandboxWsClient.connect({
        gatewayHttpUrl: config.SANDBOX_GATEWAY_HTTP_URL,
        gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
        apiKey: config.SANDBOX_API_KEY,
      });
      connections.add(client);
      const leasing = client.lease.bind(client);
      context.mock.method(client, 'lease', async (...args: Parameters<typeof leasing>) => {
        const grant = await leasing(...args);
        if (grant.created === true) owned.add(grant.lease_id);
        return grant;
      });
      return client;
    }
    context.after(async () => {
      try {
        if (owned.size > 0) {
          const cleanup = await connect();
          const present = (await cleanup.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id)));
          const results = await Promise.allSettled(present.map((row) => cleanup.deleteLease(leaseId(row.lease_id))));
          assert.ok(
            results.every((entry) => entry.status === 'fulfilled'),
            'delete every observed newly-created persistence lease',
          );
          assert.equal((await cleanup.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id))).length, 0);
        }
      } finally {
        for (const client of connections) client.close();
      }
    });

    const marker = z.string().uuid().parse(crypto.randomUUID());
    const name = 'persistence-' + marker;
    const path = '/tmp/' + name;
    const tags = { fixture: 'named-server-continuity', marker };
    const first = await connect();
    const source = await Sandbox.create(first, {
      tenant: tenantId(config.SANDBOX_TENANT),
      providerPref: ['local_docker'],
      name,
      tags,
      persistent: false,
      timeoutMs: 180_000,
      spec: {
        image: {
          kind: 'registry',
          reference: { registry: 'docker.io', repository: 'library/node', tag: '22-bookworm-slim' },
        },
        workspace: { empty: {} },
        env: {},
        cpu_seconds: 60,
        memory_mib: 256,
        boot_mode: 'bare',
        working_dir: '/tmp',
        network_policy: 'deny_all',
      },
    });
    assert.equal(source.created, true, 'the fixture must own its newly-created grant');
    assert.ok(owned.has(source.sandboxId));
    const serverSource = String.raw`
import { createServer } from 'node:http';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
const path = ${JSON.stringify(path + '.json')};
const previous = existsSync(path) ? JSON.parse(readFileSync(path, 'utf8')) : undefined;
const stat = readFileSync('/proc/' + process.pid + '/stat', 'utf8');
const state = {
  marker: ${JSON.stringify(marker)}, pid: process.pid,
  start: stat.slice(stat.lastIndexOf(')') + 2).trim().split(/\s+/)[19],
  launches: (previous?.launches ?? 0) + 1,
};
writeFileSync(path, JSON.stringify(state));
createServer((_request, response) => {
  response.writeHead(200, { 'content-type': 'application/json' });
  response.end(JSON.stringify(state));
}).listen(3000, '127.0.0.1');
`;
    await source.writeFiles([{ path: path + '.mjs', content: serverSource }]);
    const server = await source.startServer({
      cmd: 'node',
      args: [path + '.mjs'],
      name: 'owned-web',
      port: 3000,
      preview: false,
      timeoutMs: 15_000,
    });
    const executionId = server.command.cmdId;
    const baselineAttachment = attachmentSchema.parse(await Sandbox.get(first, { name }));

    async function liveMarker(sandbox: SandboxHandle) {
      const code = `(async()=>{const response=await fetch('http://127.0.0.1:3000/',{signal:AbortSignal.timeout(3000)});if(response.status!==200)throw new Error('HTTP readiness failed');process.stdout.write(await response.text())})().catch(()=>{process.stderr.write('continuity probe failed');process.exitCode=1})`;
      const response = await sandbox.runCommand({ cmd: 'node', args: ['-e', code], timeoutSeconds: 5 });
      const result = commandOutputSchema.parse({
        exitCode: response.exitCode,
        stdout: response.stdout(),
        stderr: response.stderr(),
        timedOut: response.timedOut,
        truncated: response.truncated,
      });
      assert.equal(result.exitCode, 0, 'the original HTTP service answers inside its sandbox');
      assert.equal(result.timedOut, false);
      assert.equal(result.truncated, false);
      const served = markerSchema.parse(JSON.parse(result.stdout));
      const stored = await sandbox.readFile({ path: path + '.json' });
      assert.ok(stored, 'the server records its actual process identity');
      assert.deepEqual(markerSchema.parse(JSON.parse(stored)), served);
      return served;
    }

    const original = await liveMarker(source);
    assert.equal(original.marker, marker);
    assert.equal(original.launches, 1);

    function assertAttachment(sandbox: SandboxHandle, state: 'running' | 'paused') {
      const attached = attachmentSchema.parse(sandbox);
      assert.equal(attached.sandboxId, baselineAttachment.sandboxId);
      assert.equal(attached.provider, baselineAttachment.provider);
      assert.equal(attached.name, name);
      assert.deepEqual(attached.tags, baselineAttachment.tags);
      for (const [key, value] of Object.entries(tags)) assert.equal(attached.tags[key], value);
      assert.equal(attached.createdAt.getTime(), baselineAttachment.createdAt.getTime());
      assert.equal(attached.state, state);
    }

    async function assertContinuity(sandbox: SandboxHandle) {
      assert.deepEqual(
        await liveMarker(sandbox),
        original,
        'the same PID, process start identity and HTTP marker survive',
      );
      const processes = await sandbox.processes();
      const named = processes.filter((process) => process.name === 'owned-web' && process.state === 'running');
      assert.equal(named.length, 1, 'exactly one named server is running');
      assert.equal(named[0]!.execution_id, executionId, 'the original execution is preserved');
      const state = serverClientStateSchema.parse(sandbox.exportClientState());
      assert.equal(state.servers.length, 1);
      assert.equal(state.servers[0]!.executionId, executionId);
      return state;
    }

    let state = serverClientStateSchema.parse(JSON.parse(JSON.stringify(source.exportClientState())));
    assert.equal(state.sourceSandboxId, source.sandboxId);
    assert.equal(state.servers.length, 1);
    first.close();
    const second = await connect();
    let attached = await Sandbox.get(second, { name, clientState: state });

    await context.test('a fresh client recovers named metadata and trusted server intent', async () => {
      assertAttachment(attached, 'running');
      state = await assertContinuity(attached);
    });
    await context.test(
      'pause is visible through a fresh handle and resume preserves the original process',
      async () => {
        const reapAt = z.date().parse(await attached.pause({ maxPausedMs: 30_000 }));
        assert.ok(reapAt.getTime() > Date.now());
        attached = await Sandbox.get(second, { name, clientState: state });
        assertAttachment(attached, 'paused');
        const deadline = z.date().parse(await attached.resume());
        assert.ok(deadline.getTime() > Date.now());
        attached = await Sandbox.get(second, { name, clientState: state });
        assertAttachment(attached, 'running');
        state = await assertContinuity(attached);
      },
    );
    await context.test(
      'another closed client can reattach and resume idempotently without launching a duplicate',
      async () => {
        state = serverClientStateSchema.parse(JSON.parse(JSON.stringify(state)));
        second.close();
        const third = await connect();
        attached = await Sandbox.get(third, { name, clientState: state });
        assertAttachment(attached, 'running');
        await attached.resume();
        await assertContinuity(attached);
      },
    );
  },
);

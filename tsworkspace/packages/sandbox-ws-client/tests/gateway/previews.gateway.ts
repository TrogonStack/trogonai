import assert from 'node:assert/strict';
import { request } from 'node:http';
import { test } from 'node:test';

import { z } from 'zod';

import type { LeaseId } from '../../src/index.ts';

const { PreviewNotReadyError, Sandbox, SandboxWsClient, leaseId, tenantId }: typeof import('../../src/index.ts') =
  await import(new URL('../../dist/index.js', import.meta.url).href);

const server = String.raw`
import { createServer } from 'node:http';
let stream;
createServer((request, response) => {
  const path = new URL(request.url, 'http://guest.invalid').pathname;
  if (path === '/redirect') {
    response.writeHead(302, { location: '/health?redirected=1' }).end();
  } else if (path === '/nested/relative') {
    response.writeHead(302, { location: '../health?relative=1' }).end();
  } else if (path === '/redirect-absolute') {
    response.writeHead(302, { location: 'http://' + request.headers.host + '/health?absolute=1' }).end();
  } else if (path === '/redirect-external') {
    response.writeHead(302, { location: 'https://example.invalid/external' }).end();
  } else if (path === '/stream') {
    if (stream) { response.writeHead(409).end(); return; }
    stream = response;
    response.writeHead(200, { 'content-type': 'text/plain' });
    response.write('stream-first\n');
  } else if (path === '/finish') {
    stream?.end('stream-last\n'); stream = undefined;
    response.end('released');
  } else { response.end('preview-ready'); }
}).listen(3000, '0.0.0.0');
`;

function upgradeStatus(url: string): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = request(url, { headers: { connection: 'Upgrade', upgrade: 'websocket' } }, (response) => {
      response.resume();
      response.once('end', () => {
        probe.destroy();
        resolve(response.statusCode ?? 0);
      });
    });
    probe.once('upgrade', (_response, socket) => {
      socket.destroy();
      resolve(101);
    });
    probe.once('error', () => reject(new Error('preview upgrade probe failed')));
    probe.setTimeout(5_000, () => probe.destroy(new Error('preview upgrade probe timed out')));
    probe.end();
  });
}

test(
  'signed local previews preserve HTTP behavior, access and expose-after-create',
  { timeout: 90_000 },
  async (context) => {
    const config = z
      .object({
        SANDBOX_GATEWAY_HTTP_URL: z.string().url(),
        SANDBOX_GATEWAY_WS_URL: z.string().url(),
        SANDBOX_API_KEY: z.string().min(1),
        SANDBOX_TENANT: z.string().min(1),
        SANDBOX_PREVIEW_TTL_SECONDS: z.coerce.number().int().min(1).max(10),
      })
      .parse(process.env);
    const client = await SandboxWsClient.connect({
      gatewayHttpUrl: config.SANDBOX_GATEWAY_HTTP_URL,
      gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
      apiKey: config.SANDBOX_API_KEY,
    });
    const owned = new Set<LeaseId>();
    const leasing = client.lease.bind(client);
    context.mock.method(client, 'lease', async (...args: Parameters<typeof leasing>) => {
      const grant = await leasing(...args);
      if (grant.created === true) owned.add(grant.lease_id);
      return grant;
    });
    context.after(async () => {
      try {
        const present = (await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id)));
        const cleanup = await Promise.allSettled(present.map((row) => client.deleteLease(leaseId(row.lease_id))));
        assert.ok(
          cleanup.every((item) => item.status === 'fulfilled'),
          'delete every observed new preview lease',
        );
        assert.equal((await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id))).length, 0);
      } finally {
        client.close();
      }
    });
    const sandbox = await Sandbox.create(client, {
      tenant: tenantId(config.SANDBOX_TENANT),
      providerPref: ['local_docker'],
      timeoutMs: 120_000,
      image: {
        kind: 'registry',
        reference: { registry: 'docker.io', repository: 'library/node', tag: '22-bookworm-slim' },
      },
      ports: [],
      networkPolicy: 'deny-all',
    });
    assert.equal(sandbox.created, true);
    await assert.rejects(
      () => sandbox.preview(3000, { waitMs: 100 }),
      (error: unknown) => error instanceof PreviewNotReadyError,
    );
    const path = '/tmp/preview-' + crypto.randomUUID() + '.mjs';
    await sandbox.writeFiles([{ path, content: server }]);
    await sandbox.startServer({ cmd: 'node', args: [path], port: 3000, preview: false });
    const fresh = () => sandbox.preview(3000, { waitMs: 2_000 });

    await context.test('an undeclared service becomes reachable through a signed preview', async () => {
      const preview = await fresh();
      const response = await fetch(new URL('health', preview.url));
      assert.equal(response.status, 200);
      assert.equal(await response.text(), 'preview-ready');
    });
    await context.test('upstream redirects retain the signed preview route', async () => {
      const preview = await fresh();
      for (const path of ['redirect', 'nested/relative', 'redirect-absolute']) {
        const response = await fetch(new URL(path, preview.url));
        assert.equal(response.status, 200, 'following a service redirect must reach the same preview');
        assert.equal(await response.text(), 'preview-ready');
      }
      const external = await fetch(new URL('redirect-external', preview.url), { redirect: 'manual' });
      assert.equal(external.status, 302);
      assert.equal(external.headers.get('location'), 'https://example.invalid/external');
    });
    await context.test('the first stream chunk arrives before the guest is allowed to finish', async () => {
      const preview = await fresh();
      const response = await fetch(new URL('stream', preview.url), { signal: AbortSignal.timeout(15_000) });
      assert.equal(response.status, 200);
      const reader = response.body!.getReader();
      const decoder = new TextDecoder();
      let text = '';
      while (!text.includes('stream-first\n')) {
        const result = await reader.read();
        assert.equal(result.done, false, 'stream must remain open before the release request');
        text += decoder.decode(result.value, { stream: true });
      }
      assert.equal(text.includes('stream-last'), false);
      assert.equal((await fetch(new URL('finish', preview.url))).status, 200);
      for (;;) {
        const result = await reader.read();
        if (result.done) break;
        text += decoder.decode(result.value, { stream: true });
      }
      text += decoder.decode();
      assert.equal(text, 'stream-first\nstream-last\n');
    });
    await context.test('invalid and wrong-port tokens are rejected and upgrades have an explicit refusal', async () => {
      const preview = await fresh();
      const wrongPort = new URL(preview.url);
      wrongPort.pathname = wrongPort.pathname.replace('/3000/', '/3001/');
      assert.equal((await fetch(wrongPort)).status, 401);
      const invalid = new URL(preview.url);
      invalid.pathname = invalid.pathname.split('/').slice(0, 5).join('/') + '/invalid-token/';
      assert.equal((await fetch(invalid)).status, 401);
      assert.equal(await upgradeStatus(preview.url), 501);
    });
    await context.test('expired previews are rejected and a renewed token works', async () => {
      const preview = await fresh();
      const remaining = preview.expiresAt.getTime() - Date.now();
      assert.ok(remaining >= 0 && remaining <= 10_000, 'the fixture needs a short configured token lifetime');
      await new Promise((resolve) => setTimeout(resolve, remaining + 100));
      assert.equal((await fetch(preview.url)).status, 401);
      const renewed = await fresh();
      assert.ok(renewed.expiresAt > preview.expiresAt);
      assert.equal((await fetch(renewed.url)).status, 200);
    });
    await context.test('deleting the lease revokes a still-current preview', async () => {
      const preview = await fresh();
      await sandbox.delete();
      assert.equal((await fetch(preview.url)).status, 403);
    });
  },
);

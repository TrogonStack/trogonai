import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

import { z } from 'zod';

import type { LeaseId } from '../../src/index.ts';

const { Sandbox, SandboxWsClient, SandboxWsError, digest, leaseId, tenantId }: typeof import('../../src/index.ts') =
  await import(new URL('../../dist/index.js', import.meta.url).href);

const imageSchema = z
  .object({
    registry: z.string(),
    repository: z.string().min(1),
    tag: z.string().min(1).optional(),
    digest: z
      .string()
      .regex(/^sha256:[0-9a-f]{64}$/)
      .transform(digest)
      .optional(),
  })
  .refine((image) => image.tag !== undefined || image.digest !== undefined, 'set an explicit image tag or digest');

const commandResultSchema = z.object({
  exitCode: z.number().int(),
  stdout: z.string(),
  stderr: z.string(),
  timedOut: z.boolean(),
});

test(
  'a requestable browser lease captures a real offline PNG and rejects a different image',
  { timeout: 120_000 },
  async (context) => {
    const config = z
      .object({
        SANDBOX_GATEWAY_HTTP_URL: z.string().url(),
        SANDBOX_GATEWAY_WS_URL: z.string().url(),
        SANDBOX_API_KEY: z.string().min(1),
        SANDBOX_TENANT: z.string().min(1),
        SANDBOX_BROWSER_IMAGE_JSON: z.string().transform((value) => imageSchema.parse(JSON.parse(value))),
      })
      .parse(process.env);
    const client = await SandboxWsClient.connect({
      gatewayHttpUrl: config.SANDBOX_GATEWAY_HTTP_URL,
      gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
      apiKey: config.SANDBOX_API_KEY,
    });
    const owned = new Set<LeaseId>();
    const lease = client.lease.bind(client);
    context.mock.method(client, 'lease', async (...args: Parameters<typeof lease>) => {
      const grant = await lease(...args);
      owned.add(grant.lease_id);
      return grant;
    });
    context.after(async () => {
      try {
        const present = (await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id)));
        const cleaned = await Promise.allSettled(present.map((row) => client.deleteLease(leaseId(row.lease_id))));
        assert.ok(
          cleaned.every((result) => result.status === 'fulfilled'),
          'remove every observed browser lease',
        );
        assert.equal((await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id))).length, 0);
      } finally {
        client.close();
      }
    });
    const sandbox = await Sandbox.create(client, {
      tenant: tenantId(config.SANDBOX_TENANT),
      providerPref: ['local_docker'],
      browser: true,
      image: { kind: 'registry', reference: config.SANDBOX_BROWSER_IMAGE_JSON },
      memoryMib: 1024,
      vcpus: 1,
      timeoutMs: 60_000,
      networkPolicy: 'deny-all',
    });
    const directory = '/tmp/browser-lease-' + crypto.randomUUID();
    let primaryFailure: unknown;
    try {
      assert.ok(sandbox.supports('browser'), 'the real lease grant must report the earned browser feature');
      const identity = await sandbox.runCommand({
        cmd: 'node',
        args: [
          '-e',
          'process.stdout.write(JSON.stringify({ node: process.versions.node, uid: process.getuid(), home: process.env.HOME }))',
        ],
      });
      const result = commandResultSchema.parse({
        exitCode: identity.exitCode,
        stdout: identity.stdout(),
        stderr: identity.stderr(),
        timedOut: identity.timedOut,
      });
      assert.equal(result.exitCode, 0);
      const runtime = z
        .object({ node: z.string(), uid: z.literal(65532), home: z.literal('/tmp') })
        .parse(JSON.parse(result.stdout));
      const [major, minor] = runtime.node.split('.').map(Number);
      assert.ok(major! > 22 || (major === 22 && minor! >= 7));
      await sandbox.mkDir(directory);
      const shot = await sandbox.runCommand({
        cmd: 'chromium',
        args: [
          '--headless=new',
          '--disable-gpu',
          '--no-sandbox',
          '--no-first-run',
          '--user-data-dir=' + directory + '/profile',
          '--screenshot=' + directory + '/page.png',
          '--window-size=320,240',
          '--timeout=10000',
          'data:text/html,<title>Browser proof</title><h1>Sandbox browser</h1>',
        ],
        timeoutSeconds: 15,
        maxOutputBytes: 16_384,
      });
      const screenshotResult = commandResultSchema.parse({
        exitCode: shot.exitCode,
        stdout: shot.stdout(),
        stderr: shot.stderr(),
        timedOut: shot.timedOut,
      });
      assert.equal(screenshotResult.exitCode, 0, screenshotResult.stderr);
      assert.equal(screenshotResult.timedOut, false);
      const png = await sandbox.readFileToBuffer({ path: directory + '/page.png' });
      assert.ok(png && png.length > 64);
      const bytes = Buffer.from(png);
      assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
      assert.equal(bytes.readUInt32BE(16), 320);
      assert.equal(bytes.readUInt32BE(20), 240);
      context.diagnostic(
        JSON.stringify({
          node: runtime.node,
          uid: runtime.uid,
          pngBytes: bytes.length,
          pngSha256: createHash('sha256').update(bytes).digest('hex'),
        }),
      );
    } catch (error) {
      primaryFailure = error;
      throw error;
    } finally {
      const files = await Promise.allSettled([sandbox.fs.rm(directory, { recursive: true })]);
      const leaseCleanup = await Promise.allSettled([sandbox.delete()]);
      const failed = [...files, ...leaseCleanup].filter((result) => result.status === 'rejected').length;
      if (primaryFailure === undefined) assert.equal(failed, 0, 'browser files and lease must be removed');
      else if (failed > 0)
        context.diagnostic(`browser cleanup had ${failed} failure(s); the primary failure is preserved`);
    }
    await assert.rejects(
      () =>
        Sandbox.create(client, {
          tenant: tenantId(config.SANDBOX_TENANT),
          providerPref: ['local_docker'],
          browser: true,
          image: {
            kind: 'registry',
            reference: { registry: 'docker.io', repository: 'library/node', tag: '22-bookworm-slim' },
          },
          memoryMib: 1024,
          vcpus: 1,
          timeoutMs: 60_000,
        }),
      (error: unknown) => error instanceof SandboxWsError && /browser.*image|image.*browser/i.test(error.message),
    );
  },
);

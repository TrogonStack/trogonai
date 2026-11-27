import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  CommandFinished,
  MemorySandboxClient,
  PreviewNotReadyError,
  Sandbox,
  SandboxWsClient,
  leaseId,
} from '@trogonai/sandbox-ws-client';

import { previewStateSchema } from '../../../contracts.ts';
import { GET } from './route.ts';

for (const probe of [
  { name: 'refused', exit: 1, out: 'ECONNREFUSED', expected: 'not_ready' },
  { name: 'unknown failure', exit: 1, out: 'ETIMEDOUT', expected: 'unavailable' },
  { name: 'responding', exit: 0, out: 'app responds', expected: 'ready' },
  {
    name: 'gateway cannot route a responding service',
    exit: 0,
    out: 'app responds',
    expected: 'unavailable',
    gatewayNotReady: true,
  },
]) {
  test(`preview checks the actual Node service when its URL exists: ${probe.name}`, async (context) => {
    const previous = new Map(
      [
        'SANDBOX_PROVIDER',
        'SANDBOX_TENANT',
        'SANDBOX_GATEWAY_HTTP_URL',
        'SANDBOX_GATEWAY_WS_URL',
        'SANDBOX_API_KEY',
      ].map((key) => [key, process.env[key]]),
    );
    Object.assign(process.env, {
      SANDBOX_PROVIDER: 'local_docker',
      SANDBOX_TENANT: 'preview-tests',
      SANDBOX_GATEWAY_HTTP_URL: 'http://gateway.invalid',
      SANDBOX_GATEWAY_WS_URL: 'ws://gateway.invalid',
      SANDBOX_API_KEY: 'test-placeholder',
    });
    context.after(() => {
      for (const [key, value] of previous) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
    });
    const client = new MemorySandboxClient({ commands: { sh: () => ({ exitCode: 0 }) } });
    context.mock.method(SandboxWsClient, 'connect', async () => client);
    const cache = (globalThis as { __appBuilder?: { client: unknown; sandboxes: Map<string, unknown> } }).__appBuilder;
    if (cache !== undefined) {
      cache.client = undefined;
      cache.sandboxes.clear();
    }
    context.mock.method(Sandbox.prototype, 'preview', async () => {
      if ('gatewayNotReady' in probe) throw new PreviewNotReadyError(3000, 2000);
      return { url: 'https://preview.invalid/app/', expiresAt: new Date(Date.now() + 60_000) };
    });
    let probes = 0;
    const runCommand = Sandbox.prototype.runCommand;
    context.mock.method(
      Sandbox.prototype,
      'runCommand',
      function (this: Sandbox, ...args: Parameters<typeof runCommand>) {
        const params = args[0];
        if (typeof params === 'object' && params.cmd === 'node') {
          probes++;
          assert.match(params.args?.join(' ') ?? '', /fetch\(/);
          return Promise.resolve(
            new CommandFinished(
              leaseId(this.sandboxId),
              'probe',
              probe.exit,
              new TextEncoder().encode(probe.out),
              new Uint8Array(),
            ),
          );
        }
        return runCommand.apply(this, args);
      },
    );
    const response = await GET(new Request('http://example.invalid/api/preview'));
    const state = previewStateSchema.parse(await response.json());
    assert.equal(probes, 1, 'URL allocation alone does not prove readiness');
    assert.equal(state.status, probe.expected);
    if (state.status === 'ready') assert.equal(state.url, 'https://preview.invalid/app/');
  });
}

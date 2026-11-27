import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { TestContext } from 'node:test';

import { MemorySandboxClient, SandboxWsClient } from '@trogonai/sandbox-ws-client';
import type { LeaseRequest } from '@trogonai/sandbox-ws-client';

import { sandboxForSession } from './sandbox.ts';

function recordingClient(context: TestContext): { requests: LeaseRequest[] } {
  const previous = new Map(
    ['SANDBOX_PROVIDER', 'SANDBOX_TENANT', 'SANDBOX_GATEWAY_HTTP_URL', 'SANDBOX_GATEWAY_WS_URL', 'SANDBOX_API_KEY'].map(
      (key) => [key, process.env[key]],
    ),
  );
  delete process.env.SANDBOX_PROVIDER;
  process.env.SANDBOX_TENANT = 'app-builder-test';
  process.env.SANDBOX_GATEWAY_HTTP_URL = 'http://gateway.invalid';
  process.env.SANDBOX_GATEWAY_WS_URL = 'ws://gateway.invalid';
  process.env.SANDBOX_API_KEY = 'test-only-placeholder';
  context.after(() => {
    for (const [key, value] of previous) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
  });

  const client = new MemorySandboxClient({ commands: { sh: () => ({ exitCode: 0 }) } });
  const lease = client.lease.bind(client);
  const requests: LeaseRequest[] = [];
  client.lease = async (request) => {
    requests.push(request);
    return lease(request);
  };
  context.mock.method(SandboxWsClient, 'connect', async () => client);

  const cache = (
    globalThis as {
      __appBuilder?: { client: unknown; sandboxes: Map<string, unknown> };
    }
  ).__appBuilder;
  if (cache !== undefined) {
    cache.client = undefined;
    cache.sandboxes.clear();
  }
  return { requests };
}

test('app-builder sends an explicit provider when using the gateway', async (context) => {
  const { requests } = recordingClient(context);
  const { sandbox } = await sandboxForSession(crypto.randomUUID());
  try {
    assert.equal(requests.length, 1);
    assert.deepEqual(requests[0]?.provider_pref, ['local_docker']);
  } finally {
    await sandbox.delete();
  }
});

for (const provider of ['', 'vercel', 'not-a-provider']) {
  test(`app-builder rejects incompatible provider configuration ${JSON.stringify(provider)} before leasing`, async (context) => {
    const { requests } = recordingClient(context);
    process.env.SANDBOX_PROVIDER = provider;
    await assert.rejects(() => sandboxForSession(crypto.randomUUID()), /SANDBOX_PROVIDER|provider/);
    assert.equal(requests.length, 0);
  });
}

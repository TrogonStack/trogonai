import { afterEach, describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, SandboxWsError, tenantId } from '../src/index.ts';
import type { StartedServer } from '../src/index.ts';

const servers: StartedServer[] = [];
afterEach(async () => {
  for (const server of servers.splice(0)) await server.stop();
});

async function setup(key: string) {
  const client = new MemorySandboxClient({
    commands: {
      serve: ({ signal }) =>
        new Promise((resolve) => {
          signal.addEventListener('abort', () => resolve({ exitCode: 143 }), { once: true });
        }),
    },
  });
  client.waitSandboxReady = async () => ({ outcome: 'ready' });
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
  servers.push(
    await sandbox.startServer({
      cmd: 'serve',
      port: 3000,
      preview: false,
      env: { [key]: 'caller-value' },
      redactValues: ['caller-value'],
    }),
  );
  const receipt = JSON.parse(JSON.stringify(await sandbox.snapshot()));
  return { client, sandbox, clientState: receipt.clientState };
}

describe('server restart environment ownership', () => {
  it.each(['toString', 'constructor'])('does not count inherited %s as a caller-provided replacement', async (key) => {
    const { client, sandbox, clientState } = await setup(key);
    const outcome = await Sandbox.connect(client, sandbox.sandboxId, { clientState }).then(
      () => 'attached',
      (error: unknown) => (error instanceof SandboxWsError ? error.code : 'unexpected_error'),
    );
    expect(outcome).toBe('invalid_request');
  });

  it('accepts an explicitly owned replacement even when its name exists on Object.prototype', async () => {
    const { client, sandbox, clientState } = await setup('constructor');
    const attached = await Sandbox.connect(client, sandbox.sandboxId, {
      clientState,
      env: { constructor: 'new-caller-value' },
    });
    expect(attached.sandboxId).toBe(sandbox.sandboxId);
  });
});

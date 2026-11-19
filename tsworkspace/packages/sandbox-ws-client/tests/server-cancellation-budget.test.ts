import { expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';

it('keeps a per-call cancellation budget outside persisted server restart intent', async () => {
  let finish!: () => void;
  const client = new MemorySandboxClient({
    commands: {
      serve: ({ signal }) =>
        new Promise((resolve) => {
          finish = () => resolve({ exitCode: 143 });
          signal.addEventListener('abort', finish, { once: true });
        }),
    },
  });
  client.waitSandboxReady = async () => ({ outcome: 'ready' });
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
  try {
    await sandbox.startServer({ cmd: 'serve', port: 3000, preview: false, cancellationTimeoutMs: 100 });
    const state = sandbox.exportClientState();
    expect(state.servers).toHaveLength(1);
    expect(state.servers[0]?.params).not.toHaveProperty('cancellationTimeoutMs');
    expect(state.servers[0]?.params.cmd).toBe('serve');
    const receipt = await sandbox.snapshot();
    expect(receipt.clientState).toEqual(state);
  } finally {
    await sandbox.delete();
    finish?.();
  }
});

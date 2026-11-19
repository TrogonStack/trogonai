import { afterEach, describe, expect, it, vi } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { StartedServer } from '../src/index.ts';

const tenant = tenantId('acme');
const servers: StartedServer[] = [];
const endings: Array<() => void> = [];
afterEach(async () => {
  for (const server of servers.splice(0)) await server.stop();
  for (const end of endings.splice(0)) end();
});

async function fixture() {
  let starts = 0;
  const client = new MemorySandboxClient({
    commands: {
      serve: ({ signal }) => {
        starts += 1;
        return new Promise((resolve) => {
          const end = () => resolve({ exitCode: 143 });
          endings.push(end);
          signal.addEventListener('abort', end, { once: true });
        });
      },
    },
  });
  client.waitSandboxReady = async () => ({ outcome: 'ready' });
  const source = await Sandbox.create(client, { tenant, name: 'source', idempotencyKey: 'source' });
  const server = await source.startServer({ cmd: 'serve', port: 3000, preview: false, name: 'web' });
  servers.push(server);
  const receipt = JSON.parse(JSON.stringify(await source.snapshot()));
  return { client, source, server, receipt, starts: () => starts };
}

describe('server restart plan attachment authority', () => {
  it('rejects another sandbox source identity on connect', async () => {
    const state = await fixture();
    const target = await Sandbox.create(state.client, { tenant, name: 'target' });
    const outcome = await Sandbox.connect(state.client, target.sandboxId, {
      clientState: state.source.exportClientState(),
    }).then(
      () => 'attached',
      (error: { code?: string }) => error.code,
    );
    expect(outcome).toBe('invalid_request');
  });

  it('rejects another sandbox source identity on named lookup', async () => {
    const state = await fixture();
    await Sandbox.create(state.client, { tenant, name: 'target' });
    const outcome = await Sandbox.get(state.client, {
      name: 'target',
      clientState: state.source.exportClientState(),
    }).then(
      () => 'attached',
      (error: { code?: string }) => error.code,
    );
    expect(outcome).toBe('invalid_request');
  });

  it.each([false, undefined])(
    'does not infer lost processes from a disk-only receipt when the grant is reused or unclassified (%s)',
    async (created) => {
      const state = await fixture();
      const lease = state.client.lease.bind(state.client);
      state.client.lease = async (request) => ({ ...(await lease(request)), created });
      state.client.listProcesses = async () => ({ outcome: 'unsupported' });
      const deleted = vi.spyOn(state.client, 'deleteLease');
      const outcome = await Sandbox.fromSnapshot(state.client, state.receipt, {
        tenant,
        idempotencyKey: 'source',
      }).then(
        () => 'restored',
        (error: { code?: string }) => error.code,
      );
      expect(outcome).toBe('unsupported');
      expect(state.starts()).toBe(1);
      expect(deleted).not.toHaveBeenCalled();
      expect(await state.client.pollCommand(state.source.sandboxId, state.server.command.cmdId)).toEqual({
        outcome: 'running',
      });
    },
  );

  it.each([false, undefined])(
    'reattaches a positively identified running process on a reused or unclassified disk-only restore (%s)',
    async (created) => {
      const state = await fixture();
      const lease = state.client.lease.bind(state.client);
      state.client.lease = async (request) => ({ ...(await lease(request)), created });
      state.client.listProcesses = async () => ({
        outcome: 'processes',
        processes: [{ execution_id: state.server.command.cmdId, name: 'web', state: 'running' }],
      });
      const attached = await Sandbox.fromSnapshot(state.client, state.receipt, { tenant, idempotencyKey: 'source' });
      expect(attached.sandboxId).toBe(state.source.sandboxId);
      expect(state.starts()).toBe(1);
    },
  );
});

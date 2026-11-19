import { describe, expect, it, vi } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';

const tenant = tenantId('acme');
const startedAt = 1_900_000_000;

async function fixture() {
  const client = new MemorySandboxClient({ now: () => startedAt * 1000 });
  const source = await Sandbox.create(client, { tenant, name: 'web', tags: { role: 'preview' } });
  return { client, source };
}

describe('reconnected sandbox metadata', () => {
  it.each(['connect', 'list'] as const)(
    'retains reported name, labels, state and creation time through %s without executing commands',
    async (operation) => {
      const { client, source } = await fixture();
      const list = client.listLeases.bind(client);
      client.listLeases = async (options) => ({
        leases: (await list(options)).leases.map((row) => ({ ...row, name: 'web', state: 'paused' })),
      });
      const run = vi.spyOn(client, 'runCommand');
      const attached =
        operation === 'connect' ? await Sandbox.connect(client, source.sandboxId) : (await Sandbox.list(client))[0]!;
      expect(attached.name).toBe('web');
      expect(attached.tags).toMatchObject({ role: 'preview' });
      expect(attached.state).toBe('paused');
      expect(attached.createdAt).toEqual(new Date(startedAt * 1000));
      expect(attached.created).toBe(false);
      expect(run).not.toHaveBeenCalled();
    },
  );

  it('retains the named lookup state and precise creation timestamp', async () => {
    const { client } = await fixture();
    const get = client.getLeaseByName.bind(client);
    client.getLeaseByName = async (name) => {
      const found = await get(name);
      return found.outcome === 'found'
        ? {
            ...found,
            row: {
              ...found.row,
              status: 'paused',
              created_at: { secs_since_epoch: startedAt, nanos_since_epoch: 123_000_000 },
            },
          }
        : found;
    };
    const attached = await Sandbox.get(client, { name: 'web' });
    expect(attached.state).toBe('paused');
    expect(attached.createdAt).toEqual(new Date(startedAt * 1000 + 123));
  });

  it('keeps legacy rows without a name attachable without inventing a name', async () => {
    const { client, source } = await fixture();
    const list = client.listLeases.bind(client);
    client.listLeases = async (options) => ({
      leases: (await list(options)).leases.map((row) => {
        const { name: _name, ...legacy } = row;
        return legacy;
      }),
    });
    const attached = await Sandbox.connect(client, source.sandboxId);
    expect(attached.name).toBeUndefined();
    expect(attached.tags).toMatchObject({ role: 'preview' });
  });

  it('models paused state in memory discovery rather than claiming the lease is running', async () => {
    const { client, source } = await fixture();
    await source.pause();
    const listed = await client.listLeases();
    expect(listed.leases[0]?.state).toBe('paused');
    const named = await client.getLeaseByName('web');
    expect(named.outcome === 'found' ? named.row.status : 'missing').toBe('paused');
  });
});

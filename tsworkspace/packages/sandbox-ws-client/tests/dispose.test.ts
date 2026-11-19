import { describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, SandboxWsError, tenantId } from '../src/index.ts';

function spyLifecycle(client: MemorySandboxClient): string[] {
  const calls: string[] = [];
  const stop = client.stopLease.bind(client);
  const del = client.deleteLease.bind(client);
  client.stopLease = (async (leaseId) => {
    calls.push('stop');
    return stop(leaseId);
  }) as typeof client.stopLease;
  client.deleteLease = (async (leaseId) => {
    calls.push('delete');
    return del(leaseId);
  }) as typeof client.deleteLease;
  return calls;
}

describe('Sandbox scoped disposal', () => {
  it('deletes an unnamed sandbox when the scope ends', async () => {
    const client = new MemorySandboxClient();
    const calls = spyLifecycle(client);
    {
      await using sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
      expect(sandbox.sandboxId).toBeDefined();
    }
    expect(calls).toEqual(['delete']);
  });

  it('releases a named sandbox with stop instead of deleting it', async () => {
    const client = new MemorySandboxClient();
    const calls = spyLifecycle(client);
    {
      await using _sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), name: 'keeper' });
    }
    expect(calls).toEqual(['stop']);
  });

  it('is safe to call twice, including concurrently', async () => {
    const client = new MemorySandboxClient();
    const calls = spyLifecycle(client);
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    await Promise.all([sandbox[Symbol.asyncDispose](), sandbox[Symbol.asyncDispose]()]);
    await sandbox[Symbol.asyncDispose]();
    expect(calls).toEqual(['delete']);
  });

  it('runs the release again after a failed attempt', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    const del = client.deleteLease.bind(client);
    let fail = true;
    client.deleteLease = (async (leaseId) => {
      if (fail) {
        fail = false;
        throw new SandboxWsError('transport', 'boom');
      }
      return del(leaseId);
    }) as typeof client.deleteLease;
    await expect(sandbox[Symbol.asyncDispose]()).rejects.toThrow('boom');
    await expect(sandbox[Symbol.asyncDispose]()).resolves.toBeUndefined();
  });
});
